//! Read-only, bounded export of inferred types. No inference rules live here.
use std::collections::HashMap;

use ruff_db::parsed::parsed_module;
use ruff_text_size::Ranged;

use super::{
    ClassLiteral, DynamicType, LiteralValueTypeKind, Type,
    tuple::{Tuple, VariableSegment},
    typed_dict::{TypedDictOpenness, TypedDictType},
};
use crate::{Db, ProgramEnvironment};

/// An index into an exported graph. References may point backwards or forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

/// A definition identity, scoped by its source and qualified Python name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub source: String,
    pub start: Option<u32>,
    pub path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: TypeId,
    pub required: bool,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Openness {
    Implicit,
    Closed,
    Extra { ty: TypeId, read_only: bool },
}

/// Unsupported is explicit: consumers must not interpret it as Any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Any,
    Unknown,
    Unsupported,
    Never,
    None,
    BoolLiteral(bool),
    IntLiteral(i64),
    StringLiteral(String),
    BytesLiteral(Vec<u8>),
    LiteralString,
    EnumLiteral {
        identity: Identity,
        member: String,
    },
    Instance {
        identity: Identity,
        arguments: Vec<TypeId>,
    },
    Protocol {
        identity: Identity,
        arguments: Vec<TypeId>,
    },
    Tuple {
        prefix: Vec<TypeId>,
        variable: Option<TypeId>,
        suffix: Vec<TypeId>,
    },
    Union(Vec<TypeId>),
    Intersection {
        positive: Vec<TypeId>,
        negative: Vec<TypeId>,
    },
    Truthy,
    Falsy,
    Record {
        identity: Option<Identity>,
        fields: Vec<Field>,
        openness: Openness,
    },
    NewType {
        identity: Identity,
        base: TypeId,
    },
    Alias(TypeId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graph {
    pub roots: Vec<TypeId>,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportLimitExceeded;

/// Export iteratively, retaining cycles and sharing. A limit failure returns no partial graph.
/// The limit bounds both distinct nodes and references; callers must bound input source separately.
pub fn export<'db>(
    db: &'db dyn Db,
    env: &ProgramEnvironment<'db>,
    roots: &[Type<'db>],
    limit: u32,
) -> Result<Graph, ExportLimitExceeded> {
    let mut builder = Builder {
        db,
        env,
        limit,
        references: 0,
        ids: HashMap::new(),
        pending: Vec::new(),
    };
    let roots = roots.iter().map(|ty| builder.intern(*ty)).collect::<Result<_, _>>()?;
    let mut nodes = Vec::new();
    while nodes.len() < builder.pending.len() {
        let ty = builder.pending[nodes.len()];
        nodes.push(builder.node(ty)?);
    }
    Ok(Graph { roots, nodes })
}

struct Builder<'a, 'db> {
    db: &'db dyn Db,
    env: &'a ProgramEnvironment<'db>,
    limit: u32,
    references: u32,
    ids: HashMap<Type<'db>, TypeId>,
    pending: Vec<Type<'db>>,
}

impl<'db> Builder<'_, 'db> {
    fn intern(&mut self, ty: Type<'db>) -> Result<TypeId, ExportLimitExceeded> {
        if self.references >= self.limit {
            return Err(ExportLimitExceeded);
        }
        self.references += 1;
        if let Some(id) = self.ids.get(&ty) {
            return Ok(*id);
        }
        let id = TypeId(u32::try_from(self.pending.len()).map_err(|_| ExportLimitExceeded)?);
        self.pending.push(ty);
        self.ids.insert(ty, id);
        Ok(id)
    }

    fn ids(&mut self, types: &[Type<'db>]) -> Result<Vec<TypeId>, ExportLimitExceeded> {
        types.iter().map(|ty| self.intern(*ty)).collect()
    }

    fn identity(&self, class: ClassLiteral<'db>) -> Identity {
        let mut path = class.qualified_name(self.db).components_excluding_self();
        path.push(class.name(self.db).to_string());
        Identity {
            source: class.file(self.db).path(self.db).to_string(),
            start: class.header_span(self.db).range().map(|r| r.start().to_u32()),
            path,
        }
    }

    fn node(&mut self, ty: Type<'db>) -> Result<Node, ExportLimitExceeded> {
        let db = self.db;
        if ty.is_none(db) {
            return Ok(Node::None);
        }
        Ok(match ty {
            Type::Dynamic(DynamicType::Any) => Node::Any,
            Type::Dynamic(_) => Node::Unknown,
            Type::Never => Node::Never,
            Type::LiteralValue(literal) => match literal.kind() {
                LiteralValueTypeKind::Bool(v) => Node::BoolLiteral(v),
                LiteralValueTypeKind::Int(v) => Node::IntLiteral(v.as_i64()),
                LiteralValueTypeKind::String(v) => Node::StringLiteral(v.value(db).to_string()),
                LiteralValueTypeKind::Bytes(v) => Node::BytesLiteral(v.value(db).to_vec()),
                LiteralValueTypeKind::LiteralString => Node::LiteralString,
                LiteralValueTypeKind::Enum(v) => Node::EnumLiteral {
                    identity: self.identity(v.enum_class(db)),
                    member: v.name(db).to_string(),
                },
            },
            Type::Union(union) => Node::Union(self.ids(union.elements(db))?),
            Type::Intersection(i) => Node::Intersection {
                positive: i
                    .iter_positive(db)
                    .map(|ty| self.intern(ty))
                    .collect::<Result<_, _>>()?,
                negative: i
                    .iter_negative(db)
                    .map(|ty| self.intern(ty))
                    .collect::<Result<_, _>>()?,
            },
            Type::AlwaysTruthy => Node::Truthy,
            Type::AlwaysFalsy => Node::Falsy,
            Type::TypeAlias(alias) => Node::Alias(self.intern(alias.value_type(db))?),
            Type::NominalInstance(instance) => {
                if let Some(spec) = instance.own_tuple_spec(db) {
                    match spec.as_ref() {
                        Tuple::Fixed(tuple) => Node::Tuple {
                            prefix: self.ids(tuple.elements_slice())?,
                            variable: None,
                            suffix: Vec::new(),
                        },
                        Tuple::Variable(tuple) => match tuple.variable() {
                            VariableSegment::Homogeneous(element) => Node::Tuple {
                                prefix: self.ids(tuple.prefix_elements())?,
                                variable: Some(self.intern(element)?),
                                suffix: self.ids(tuple.suffix_elements())?,
                            },
                            VariableSegment::TypeVarTuple(_) => Node::Unsupported,
                        },
                    }
                } else {
                    let (class, spec) = instance.class(db, self.env).class_literal_and_specialization(db);
                    // Top/Bottom materializations are not ordinary generic instances.
                    // Never erase that distinction into the same type arguments.
                    if spec.is_some_and(|s| s.materialization_kind(db).is_some()) {
                        return Ok(Node::Unsupported);
                    }
                    Node::Instance {
                        identity: self.identity(class),
                        arguments: match spec {
                            Some(s) => self.ids(s.types(db))?,
                            None => Vec::new(),
                        },
                    }
                }
            }
            Type::ProtocolInstance(protocol) => {
                if protocol.materialization_kind(db).is_some() {
                    return Ok(Node::Unsupported);
                }
                let Some(origin) = protocol.class_origin(db) else {
                    return Ok(Node::Unsupported);
                };
                let (class, spec) = origin.class_literal_and_specialization(db);
                Node::Protocol {
                    identity: self.identity(class),
                    arguments: match spec {
                        Some(spec) => self.ids(spec.types(db))?,
                        None => Vec::new(),
                    },
                }
            }
            Type::TypedDict(record) => {
                let identity = match record {
                    TypedDictType::Class(class) => Some(self.identity(class.class_literal(db))),
                    TypedDictType::Synthesized(_) => None,
                };
                let mut fields = Vec::new();
                for (name, field) in record.items(db).iter() {
                    fields.push(Field {
                        name: name.to_string(),
                        ty: self.intern(field.declared_ty)?,
                        required: field.is_required(),
                        read_only: field.is_read_only(),
                    });
                }
                let openness = match record.openness(db) {
                    TypedDictOpenness::ImplicitlyOpen => Openness::Implicit,
                    TypedDictOpenness::Closed => Openness::Closed,
                    TypedDictOpenness::Extra(extra) => Openness::Extra {
                        ty: self.intern(extra.declared_ty)?,
                        read_only: extra.is_read_only(),
                    },
                };
                Node::Record {
                    identity,
                    fields,
                    openness,
                }
            }
            Type::NewTypeInstance(nominal) => {
                let definition = nominal.definition(db);
                let source = definition.file(db).path(db).to_string();
                // Definition location distinguishes equal names in separate scopes.
                let path = vec![nominal.name(db).to_string()];
                let module = parsed_module(db, definition.python_file(db)).load(db);
                let start = Some(definition.full_range(db, &module).range().start().to_u32());
                Node::NewType {
                    identity: Identity { source, start, path },
                    base: self.intern(nominal.base(db).instance_type(db, self.env))?,
                }
            }
            _ => Node::Unsupported,
        })
    }
}

/// Export the checker types of a function's returns, including implicit None.
/// This is a query over existing inference and reachability, not a second checker.
pub fn function_returns(
    db: &dyn Db,
    file: ruff_db::files::File,
    function: &ruff_python_ast::StmtFunctionDef,
    limit: u32,
) -> Result<Graph, ExportLimitExceeded> {
    use crate::{HasType, SemanticModel, reachability::ReachabilityConstraintsExtension};
    use ruff_python_ast::{
        Stmt,
        visitor::{self, Visitor},
    };
    use ty_python_core::{scope::NodeWithScopeRef, semantic_index};
    #[derive(Default)]
    struct Returns<'a> {
        values: Vec<&'a ruff_python_ast::StmtReturn>,
        depth: usize,
        exceeded: bool,
    }
    impl<'a> Visitor<'a> for Returns<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.depth >= 128 {
                self.exceeded = true;
                return;
            }
            self.depth += 1;
            match stmt {
                Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
                Stmt::Return(ret) => self.values.push(ret),
                _ => visitor::walk_stmt(self, stmt),
            }
            self.depth -= 1;
        }
    }
    let program_file = db.program_file(file);
    let env = ProgramEnvironment::from_file(program_file);
    let model = SemanticModel::new(db, program_file);
    let index = semantic_index(db, program_file);
    let scope = index.node_scope(NodeWithScopeRef::Function(function));
    if scope.is_generator_function(&index) {
        return Ok(Graph {
            roots: vec![TypeId(0)],
            nodes: vec![Node::Unsupported],
        });
    }
    let use_def = index.use_def_map(scope);
    let implicit_none = !use_def
        .reachability_constraints()
        .evaluate(db, use_def.predicates(), use_def.end_of_scope_reachability())
        .is_always_false();
    let mut returns = Returns::default();
    returns.visit_body(&function.body);
    if returns.exceeded || returns.values.len() > 1024 {
        return Err(ExportLimitExceeded);
    }
    let mut types = returns
        .values
        .into_iter()
        .filter(|ret| crate::reachability::is_range_reachable(db, &index, scope, ret.range()))
        .map(|ret| match ret.value.as_deref() {
            Some(expr) => expr.inferred_type(&model).unwrap_or(Type::unknown()),
            None => Type::none(db, &env),
        })
        .collect::<Vec<_>>();
    if implicit_none {
        types.push(Type::none(db, &env));
    }
    if types.is_empty() {
        types.push(Type::Never);
    }
    export(db, &env, &types, limit)
}

/// Export actual Python lexical bindings, including bindings in loops, patterns,
/// annotations and nested definitions, without reconstructing binding syntax.
pub fn function_locals(
    db: &dyn Db,
    file: ruff_db::files::File,
    function: &ruff_python_ast::StmtFunctionDef,
) -> Vec<String> {
    use ty_python_core::{scope::NodeWithScopeRef, semantic_index};
    let index = semantic_index(db, db.program_file(file));
    let scope = index.node_scope(NodeWithScopeRef::Function(function));
    index
        .place_table(scope)
        .symbols()
        .filter(|symbol| symbol.is_local())
        .map(|symbol| symbol.name().to_string())
        .collect()
}

/// Unbound lexical loads in an expression unit. Builtins remain external: the
/// embedding chooses which external identities denote its own dependencies.
pub fn external_names(db: &dyn Db, file: ruff_db::files::File) -> Result<Vec<(u32, u32, String)>, ExportLimitExceeded> {
    use crate::SemanticModel;
    use ruff_python_ast::{
        Expr,
        visitor::{self, Visitor},
    };
    use ty_python_core::semantic_index;
    struct Loads<'a> {
        names: Vec<&'a ruff_python_ast::ExprName>,
        depth: usize,
        exceeded: bool,
    }
    impl<'a> Visitor<'a> for Loads<'a> {
        fn visit_expr(&mut self, expr: &'a Expr) {
            if self.depth >= 128 {
                self.exceeded = true;
                return;
            }
            if let Expr::Name(name) = expr {
                if name.ctx == ruff_python_ast::ExprContext::Load {
                    self.names.push(name);
                }
            }
            self.depth += 1;
            visitor::walk_expr(self, expr);
            self.depth -= 1;
        }
    }
    let file = db.program_file(file);
    let parsed = parsed_module(db, file.python_file(db)).load(db);
    let model = SemanticModel::new(db, file);
    let index = semantic_index(db, file);
    let mut loads = Loads {
        names: vec![],
        depth: 0,
        exceeded: false,
    };
    loads.visit_body(parsed.suite());
    if loads.exceeded {
        return Err(ExportLimitExceeded);
    }
    Ok(loads
        .names
        .into_iter()
        .filter(|name| {
            !model.scope((*name).into()).is_some_and(|scope| {
                index.visible_ancestor_scopes(scope).any(|(scope, _)| {
                    index
                        .place_table(scope)
                        .symbol_by_name(name.id.as_str())
                        .is_some_and(|symbol| symbol.is_local())
                })
            })
        })
        .map(|name| (name.start().to_u32(), name.end().to_u32(), name.id.to_string()))
        .collect())
}

/// An explicit argument's parameter, obtained from the Python call binder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentBinding {
    pub parameter: String,
    pub variadic: bool,
}

pub fn bind_one_positional(
    db: &dyn Db,
    file: ruff_db::files::File,
    function: &ruff_python_ast::StmtFunctionDef,
) -> Result<ArgumentBinding, String> {
    let bindings = bind_arguments(db, file, function, &[None])?;
    bindings
        .into_iter()
        .next()
        .ok_or_else(|| "missing positional binding".into())
}

/// Bind explicit positional and named arguments in source evaluation order.
/// `None` denotes a positional argument; `Some` denotes a keyword.
pub fn bind_arguments(
    db: &dyn Db,
    file: ruff_db::files::File,
    function: &ruff_python_ast::StmtFunctionDef,
    arguments: &[Option<String>],
) -> Result<Vec<ArgumentBinding>, String> {
    use super::call::{Argument, CallArguments};
    use super::signatures::ParameterKind;
    use crate::{HasType, SemanticModel};
    let program_file = db.program_file(file);
    let model = SemanticModel::new(db, program_file);
    let env = ProgramEnvironment::from_file(program_file);
    let callable = function.inferred_type(&model).ok_or("missing callable type")?;
    let args: CallArguments = arguments
        .iter()
        .map(|name| {
            (
                name.as_deref().map_or(Argument::Positional, Argument::Keyword),
                Some(Type::unknown()),
            )
        })
        .collect();
    let bindings = callable.bindings(db, &env).match_parameters(db, &env, &args);
    let mut matches = bindings.iter_flat().flat_map(|b| b.matching_overloads());
    let (_, binding) = matches
        .next()
        .ok_or("Python arguments do not match the callable signature")?;
    if matches.next().is_some() {
        return Err("ambiguous Python call binding".into());
    }
    if !binding.errors().is_empty() {
        return Err(format!("Python call binding: {:?}", binding.errors()));
    }
    binding
        .argument_matches()
        .iter()
        .map(|argument| {
            let [matched] = argument.parameters.as_slice() else {
                return Err("ambiguous argument binding".into());
            };
            let parameter = &binding.signature.parameters()[matched.index];
            Ok(ArgumentBinding {
                parameter: parameter.name().ok_or("unnamed parameter")?.to_string(),
                variadic: matches!(
                    parameter.kind(),
                    ParameterKind::Variadic { .. } | ParameterKind::KeywordVariadic { .. }
                ),
            })
        })
        .collect()
}

/// Reachability is read from the semantic index; the embedding never decides
/// whether a Python statement or function fallthrough is reachable.
pub fn reachable_statements(
    db: &dyn Db,
    file: ruff_db::files::File,
    function: &ruff_python_ast::StmtFunctionDef,
) -> Result<(Vec<(u32, u32)>, bool), ExportLimitExceeded> {
    use crate::reachability::ReachabilityConstraintsExtension;
    use ruff_python_ast::{
        Stmt,
        visitor::{self, Visitor},
    };
    use ty_python_core::{scope::NodeWithScopeRef, semantic_index};
    struct Statements<'a> {
        values: Vec<&'a Stmt>,
        depth: usize,
        exceeded: bool,
    }
    impl<'a> Visitor<'a> for Statements<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.depth >= 128 || self.values.len() >= 4096 {
                self.exceeded = true;
                return;
            }
            self.values.push(stmt);
            if !matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
                self.depth += 1;
                visitor::walk_stmt(self, stmt);
                self.depth -= 1;
            }
        }
    }
    let index = semantic_index(db, db.program_file(file));
    let scope = index.node_scope(NodeWithScopeRef::Function(function));
    let mut statements = Statements {
        values: vec![],
        depth: 0,
        exceeded: false,
    };
    statements.visit_body(&function.body);
    if statements.exceeded {
        return Err(ExportLimitExceeded);
    }
    let ranges = statements
        .values
        .into_iter()
        .filter(|s| crate::reachability::is_range_reachable(db, &index, scope, s.range()))
        .map(|s| (s.start().to_u32(), s.end().to_u32()))
        .collect();
    let use_def = index.use_def_map(scope);
    let fallthrough = !use_def
        .reachability_constraints()
        .evaluate(db, use_def.predicates(), use_def.end_of_scope_reachability())
        .is_always_false();
    Ok((ranges, fallthrough))
}

/// Definite truth of a condition, if established by the upstream checker.
pub fn condition_truth(db: &dyn Db, file: ruff_db::files::File, expression: &ruff_python_ast::Expr) -> Option<bool> {
    use crate::{HasType, SemanticModel};
    let file = db.program_file(file);
    let model = SemanticModel::new(db, file);
    match expression
        .inferred_type(&model)?
        .bool(db, &ProgramEnvironment::from_file(file))
    {
        ty_python_core::Truthiness::AlwaysTrue => Some(true),
        ty_python_core::Truthiness::AlwaysFalse => Some(false),
        ty_python_core::Truthiness::Ambiguous => None,
    }
}
