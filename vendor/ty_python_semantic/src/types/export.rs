//! Read-only, bounded export of inferred types. No inference rules live here.
pub use super::call::bind::InvalidArgumentTypeProvenance;
pub use super::callable::CallableTypeKind;
pub use super::constraints::{
    RejectionConstraintAtom, RejectionConstraintGraph, RejectionConstraintNode, RejectionConstraintProvenance,
};
pub use super::signatures::{ParameterAnnotationKind, ParameterNamePrefix};
pub use super::typevar::TypeVarKind;
pub use super::variance::TypeVarVariance;
pub use ruff_python_parser::ParseError;
use std::collections::HashMap;

/// A parameter's raw name and declaration kind, not its rendered diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterNameEvidence {
    pub name: String,
    pub prefix: ParameterNamePrefix,
}

/// Exact owned scalar metadata from the call binder's parameter context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterEvidence {
    pub name: Option<ParameterNameEvidence>,
    pub signature_parameter_index: usize,
    pub source_parameter_index: Option<usize>,
    pub positional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgumentBindingError {
    #[error("missing positional binding")]
    MissingPositionalBinding,
    #[error("missing callable type")]
    MissingCallableType,
    #[error("ambiguous Python call binding")]
    AmbiguousCall,
    #[error("Python call binding rejected: {faults:?}")]
    CallRejected {
        faults: Vec<CallBindingFault>,
        types: RejectionTypeGraph,
    },
    #[error("call rejection type evidence exceeds the export limit: {source}")]
    EvidenceLimit {
        #[source]
        source: ExportLimitExceeded,
    },
    #[error("ambiguous argument binding at argument {argument}")]
    AmbiguousArgument { argument: usize },
    #[error("unnamed parameter at argument {argument}")]
    UnnamedParameter { argument: usize },
}

/// Owned semantic evidence from database-borrowed call-binder errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallBindingFault {
    InvalidArgumentType {
        parameter: ParameterEvidence,
        argument: Option<usize>,
        last_argument: Option<usize>,
        provenance: InvalidArgumentTypeProvenance,
        expected: TypeId,
        provided: TypeId,
        parameter_source: Option<ForwardedParameterEvidence>,
    },
    InvalidKeyType {
        argument: Option<usize>,
        provided: TypeId,
    },
    MissingArguments {
        parameters: Vec<ParameterEvidence>,
        paramspec: Option<TypeId>,
    },
    UnknownArgument {
        name: String,
        argument: Option<usize>,
    },
    UnknownKeywordVariadicArgument {
        argument: Option<usize>,
    },
    PositionalOnlyAsKeyword {
        parameter: ParameterEvidence,
        argument: Option<usize>,
    },
    TooManyPositionalArguments {
        first_excess: Option<usize>,
        expected: usize,
        provided: usize,
    },
    ParameterAlreadyAssigned {
        parameter: ParameterEvidence,
        argument: Option<usize>,
    },
    SpecializationBound {
        provided: TypeId,
        typevar: TypeId,
        argument: Option<usize>,
    },
    SpecializationConstraint {
        provided: TypeId,
        typevar: TypeId,
        argument: Option<usize>,
    },
    PropertyGetterMissing {
        property: TypeId,
    },
    PropertySetterMissing {
        property: TypeId,
    },
    PropertyDeleterMissing {
        property: TypeId,
    },
    PropertyGetterCall {
        source: Box<PropertyAccessorEvidence>,
    },
    PropertySetterCall {
        source: Box<PropertyAccessorEvidence>,
    },
    InternalCall {
        operation: &'static str,
    },
    UnmatchedOverload,
    TopCallable {
        callable: TypeId,
    },
    DataclassNamedTuple,
    DataclassTypedDict,
    DataclassEnum,
    DataclassProtocol,
    DataclassOrderRequiresEq,
    DataclassWeakrefSlotRequiresSlots,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardedParameterEvidence {
    pub function: TypeId,
    pub is_bound_method: bool,
    pub parameter_index_offset: usize,
    pub overload_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("property accessor call rejected at argument offset {argument_index_offset}: {overloads:?}")]
pub struct PropertyAccessorEvidence {
    pub callable: TypeId,
    pub argument_index_offset: usize,
    /// Preserve each callable group and each overload, including empty ones.
    pub overloads: Vec<Vec<Vec<CallBindingFault>>>,
}

use ruff_db::parsed::parsed_module;
use ruff_text_size::Ranged;

use super::{
    ClassLiteral, DynamicType, KnownClass, LiteralValueTypeKind, Type,
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

/// Diagnostic-only evidence. These nodes do not extend materialized inference values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectionTypeGraph {
    pub roots: Vec<TypeId>,
    pub nodes: Vec<RejectionTypeNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionTypeNode {
    Value(Node),
    FunctionLiteral {
        identity: Identity,
        signatures: Vec<RejectionSignature>,
    },
    Callable {
        kind: CallableTypeKind,
        signatures: Vec<RejectionSignature>,
    },
    PropertyInstance {
        class: RejectionPropertyClass,
        getter: Option<TypeId>,
        setter: Option<TypeId>,
        deleter: Option<TypeId>,
    },
    TypeVar {
        name: String,
        kind: TypeVarKind,
        definition: Option<Identity>,
        binding: Option<Identity>,
        synthetic_binding: bool,
        freshness: u32,
        paramspec_attr: Option<RejectionParamSpecAttr>,
        variance: Option<TypeVarVariance>,
        bound_or_constraints: Option<RejectionTypeVarBounds>,
        default: Option<TypeId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionPropertyClass {
    Builtin,
    Enum,
    Subclass(TypeId),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionParamSpecAttr {
    Args,
    Kwargs,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionTypeVarBounds {
    UpperBound(TypeId),
    Constraints(Vec<TypeId>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionParameterKind {
    PositionalOnly,
    PositionalOrKeyword,
    Variadic,
    KeywordOnly,
    KeywordVariadic,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionParametersKind {
    Standard,
    Gradual,
    Top,
    ParamSpec(TypeId),
    ConcatenateGradual,
    ConcatenateParamSpec(TypeId),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectionParameter {
    pub name: Option<String>,
    pub kind: RejectionParameterKind,
    pub annotation: TypeId,
    pub annotation_kind: ParameterAnnotationKind,
    pub default: Option<TypeId>,
    pub definition: Option<Identity>,
    pub source_parameter_index: Option<usize>,
    pub inferred_annotation: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectionSignature {
    pub definition: Option<Identity>,
    pub parameters_kind: RejectionParametersKind,
    pub parameters: Vec<RejectionParameter>,
    pub return_type: TypeId,
    pub generic_variables: Vec<TypeId>,
    pub is_paramspec_value: bool,
    pub source_overload_index: Option<u32>,
    pub receiver_constraints: Option<RejectionConstraintGraph<TypeId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportLimitExceeded;

impl std::fmt::Display for ExportLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("semantic export limit exceeded")
    }
}
impl std::error::Error for ExportLimitExceeded {}

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
    fn finish_rejection_graph(&mut self) -> Result<RejectionTypeGraph, ExportLimitExceeded> {
        let roots = (0..self.pending.len()).map(|index| TypeId(index as u32)).collect();
        let mut nodes = Vec::new();
        while nodes.len() < self.pending.len() {
            nodes.push(self.rejection_node(self.pending[nodes.len()])?);
        }
        Ok(RejectionTypeGraph { roots, nodes })
    }
    fn definition_identity(&self, definition: ty_python_core::definition::Definition<'db>) -> Identity {
        let db = self.db;
        let parsed = parsed_module(db, definition.python_file(db)).load(db);
        Identity {
            source: definition.file(db).path(db).to_string(),
            start: Some(definition.full_range(db, &parsed).range().start().to_u32()),
            path: definition.name(db).into_iter().collect(),
        }
    }

    fn rejection_signatures(
        &mut self,
        signatures: &super::signatures::CallableSignature<'db>,
    ) -> Result<Vec<RejectionSignature>, ExportLimitExceeded> {
        use super::signatures::{ConcatenateTail, ParameterKind, ParametersKind};
        signatures
            .iter()
            .map(|signature| {
                if self.references >= self.limit {
                    return Err(ExportLimitExceeded);
                }
                self.references += 1;
                let parameters_kind = match signature.parameters().kind() {
                    ParametersKind::Standard => RejectionParametersKind::Standard,
                    ParametersKind::Gradual => RejectionParametersKind::Gradual,
                    ParametersKind::Top => RejectionParametersKind::Top,
                    ParametersKind::ParamSpec(typevar) => {
                        RejectionParametersKind::ParamSpec(self.intern(Type::TypeVar(typevar))?)
                    }
                    ParametersKind::Concatenate(ConcatenateTail::Gradual) => {
                        RejectionParametersKind::ConcatenateGradual
                    }
                    ParametersKind::Concatenate(ConcatenateTail::ParamSpec(typevar)) => {
                        RejectionParametersKind::ConcatenateParamSpec(self.intern(Type::TypeVar(typevar))?)
                    }
                };
                let parameters = signature
                    .parameters()
                    .iter()
                    .map(|parameter| {
                        let kind = match parameter.kind() {
                            ParameterKind::PositionalOnly { .. } => RejectionParameterKind::PositionalOnly,
                            ParameterKind::PositionalOrKeyword { .. } => RejectionParameterKind::PositionalOrKeyword,
                            ParameterKind::Variadic { .. } => RejectionParameterKind::Variadic,
                            ParameterKind::KeywordOnly { .. } => RejectionParameterKind::KeywordOnly,
                            ParameterKind::KeywordVariadic { .. } => RejectionParameterKind::KeywordVariadic,
                        };
                        Ok(RejectionParameter {
                            name: parameter.name().map(|name| name.as_str().to_owned()),
                            kind,
                            annotation: self.intern(parameter.annotated_type())?,
                            annotation_kind: parameter.rejection_annotation_kind(),
                            default: parameter.default_type(self.db).map(|ty| self.intern(ty)).transpose()?,
                            definition: parameter
                                .definition()
                                .map(|definition| self.definition_identity(definition)),
                            source_parameter_index: parameter.source_parameter_index(),
                            inferred_annotation: parameter.inferred_annotation,
                        })
                    })
                    .collect::<Result<_, ExportLimitExceeded>>()?;
                let generic_variables = signature
                    .generic_context
                    .map(|context| {
                        context
                            .variables(self.db)
                            .map(|typevar| self.intern(Type::TypeVar(typevar)))
                            .collect::<Result<Vec<_>, ExportLimitExceeded>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                let (is_paramspec_value, source_overload_index) = signature.rejection_evidence_flags();
                let remaining = self.limit.checked_sub(self.references).ok_or(ExportLimitExceeded)?;
                let remaining = usize::try_from(remaining).map_err(|_| ExportLimitExceeded)?;
                let receiver_constraints = signature
                    .receiver_constraints()
                    .map(|constraints| {
                        constraints.rejection_projection(remaining, |ty| self.intern(ty), || ExportLimitExceeded)
                    })
                    .transpose()?;
                if let Some(graph) = &receiver_constraints {
                    let node_count = u32::try_from(graph.nodes.len()).map_err(|_| ExportLimitExceeded)?;
                    self.references = self
                        .references
                        .checked_add(node_count)
                        .filter(|references| *references <= self.limit)
                        .ok_or(ExportLimitExceeded)?;
                }
                Ok(RejectionSignature {
                    definition: signature
                        .definition()
                        .map(|definition| self.definition_identity(definition)),
                    parameters_kind,
                    parameters,
                    return_type: self.intern(signature.return_ty)?,
                    generic_variables,
                    is_paramspec_value,
                    source_overload_index,
                    receiver_constraints,
                })
            })
            .collect()
    }

    fn rejection_node(&mut self, ty: Type<'db>) -> Result<RejectionTypeNode, ExportLimitExceeded> {
        let db = self.db;
        Ok(match ty {
            Type::FunctionLiteral(function) => RejectionTypeNode::FunctionLiteral {
                identity: self.definition_identity(function.definition(db)),
                signatures: self.rejection_signatures(function.signature(db))?,
            },
            Type::Callable(callable) => RejectionTypeNode::Callable {
                kind: callable.kind(db),
                signatures: self.rejection_signatures(callable.signatures(db))?,
            },
            Type::PropertyInstance(property) => {
                let class = match property.instance_class(db) {
                    super::PropertyInstanceClass::Builtin => RejectionPropertyClass::Builtin,
                    super::PropertyInstanceClass::Enum => RejectionPropertyClass::Enum,
                    super::PropertyInstanceClass::Subclass(class) => {
                        RejectionPropertyClass::Subclass(self.intern(class.into())?)
                    }
                };
                RejectionTypeNode::PropertyInstance {
                    class,
                    getter: property.getter(db).map(|ty| self.intern(ty)).transpose()?,
                    setter: property.setter(db).map(|ty| self.intern(ty)).transpose()?,
                    deleter: property.deleter(db).map(|ty| self.intern(ty)).transpose()?,
                }
            }
            Type::TypeVar(typevar) => {
                let unbound = typevar.typevar(db);
                let identity = unbound.identity(db);
                let bound_or_constraints = unbound
                    .bound_or_constraints(db, self.env)
                    .map(|bounds| {
                        Ok::<_, ExportLimitExceeded>(match bounds {
                            super::TypeVarBoundOrConstraints::UpperBound(bound) => {
                                RejectionTypeVarBounds::UpperBound(self.intern(bound)?)
                            }
                            super::TypeVarBoundOrConstraints::Constraints(constraints) => {
                                RejectionTypeVarBounds::Constraints(self.ids(constraints.elements(db))?)
                            }
                        })
                    })
                    .transpose()?;
                let binding_definition = typevar.binding_context(db).definition();
                RejectionTypeNode::TypeVar {
                    name: typevar.name(db).as_str().to_owned(),
                    kind: typevar.kind(db),
                    definition: identity
                        .definition(db)
                        .map(|definition| self.definition_identity(definition)),
                    binding: binding_definition.map(|definition| self.definition_identity(definition)),
                    synthetic_binding: binding_definition.is_none(),
                    freshness: typevar.freshness(db).value(),
                    paramspec_attr: typevar.paramspec_attr(db).map(|attr| match attr {
                        super::typevar::ParamSpecAttrKind::Args => RejectionParamSpecAttr::Args,
                        super::typevar::ParamSpecAttrKind::Kwargs => RejectionParamSpecAttr::Kwargs,
                    }),
                    variance: unbound.explicit_variance(db),
                    bound_or_constraints,
                    default: unbound
                        .default_type(db, self.env)
                        .map(|ty| self.intern(ty))
                        .transpose()?,
                }
            }
            _ => RejectionTypeNode::Value(self.node(ty)?),
        })
    }

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
) -> Result<ArgumentBinding, ArgumentBindingError> {
    let bindings = bind_arguments(db, file, function, &[None])?;
    bindings
        .into_iter()
        .next()
        .ok_or(ArgumentBindingError::MissingPositionalBinding)
}

/// Bind explicit positional and named arguments in source evaluation order.
/// `None` denotes a positional argument; `Some` denotes a keyword.
pub fn bind_arguments(
    db: &dyn Db,
    file: ruff_db::files::File,
    function: &ruff_python_ast::StmtFunctionDef,
    arguments: &[Option<String>],
) -> Result<Vec<ArgumentBinding>, ArgumentBindingError> {
    use super::call::{Argument, CallArguments};
    use super::signatures::ParameterKind;
    use crate::{HasType, SemanticModel};
    let program_file = db.program_file(file);
    let model = SemanticModel::new(db, program_file);
    let env = ProgramEnvironment::from_file(program_file);
    let callable = function
        .inferred_type(&model)
        .ok_or(ArgumentBindingError::MissingCallableType)?;
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
    let Some((_, binding)) = matches.next() else {
        return Err(rejected_call(
            db,
            &env,
            bindings.iter_flat().flatten().flat_map(|binding| binding.errors()),
        ));
    };
    if matches.next().is_some() {
        return Err(ArgumentBindingError::AmbiguousCall);
    }
    if !binding.errors().is_empty() {
        return Err(rejected_call(db, &env, binding.errors()));
    }
    binding
        .argument_matches()
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let [matched] = argument.parameters.as_slice() else {
                return Err(ArgumentBindingError::AmbiguousArgument { argument: index });
            };
            let parameter = &binding.signature.parameters()[matched.index];
            Ok(ArgumentBinding {
                parameter: parameter
                    .name()
                    .ok_or(ArgumentBindingError::UnnamedParameter { argument: index })?
                    .to_string(),
                variadic: matches!(
                    parameter.kind(),
                    ParameterKind::Variadic { .. } | ParameterKind::KeywordVariadic { .. }
                ),
            })
        })
        .collect()
}

fn export_parameter_evidence(parameter: super::call::bind::ParameterContextEvidence<'_>) -> ParameterEvidence {
    ParameterEvidence {
        name: parameter.name.map(|(name, prefix)| ParameterNameEvidence {
            name: name.as_str().to_owned(),
            prefix,
        }),
        signature_parameter_index: parameter.signature_parameter_index,
        source_parameter_index: parameter.source_parameter_index,
        positional: parameter.positional,
    }
}

fn export_binding_fault<'db>(
    builder: &mut Builder<'_, 'db>,
    error: &super::call::bind::BindingError<'db>,
    depth: usize,
) -> Result<CallBindingFault, ExportLimitExceeded> {
    use super::call::bind::BindingError;
    if depth >= 128 || builder.references >= builder.limit {
        return Err(ExportLimitExceeded);
    }
    builder.references += 1;
    Ok(match error {
        BindingError::InvalidArgumentType {
            parameter,
            argument_index,
            last_argument_index,
            provenance,
            expected_ty,
            provided_ty,
            parameter_source,
        } => CallBindingFault::InvalidArgumentType {
            parameter: export_parameter_evidence(parameter.evidence()),
            argument: *argument_index,
            last_argument: *last_argument_index,
            provenance: *provenance,
            expected: builder.intern(*expected_ty)?,
            provided: builder.intern(*provided_ty)?,
            parameter_source: parameter_source
                .map(|source| {
                    let (function, is_bound_method, parameter_index_offset, overload_index) = source.evidence_parts();
                    Ok::<ForwardedParameterEvidence, ExportLimitExceeded>(ForwardedParameterEvidence {
                        function: builder.intern(Type::FunctionLiteral(function))?,
                        is_bound_method,
                        parameter_index_offset,
                        overload_index,
                    })
                })
                .transpose()?,
        },
        BindingError::InvalidKeyType {
            argument_index,
            provided_ty,
        } => CallBindingFault::InvalidKeyType {
            argument: *argument_index,
            provided: builder.intern(*provided_ty)?,
        },
        BindingError::MissingArguments { parameters, paramspec } => {
            let parameters = parameters.evidence();
            let parameter_count = u32::try_from(parameters.len()).map_err(|_| ExportLimitExceeded)?;
            builder.references = builder
                .references
                .checked_add(parameter_count)
                .filter(|references| *references <= builder.limit)
                .ok_or(ExportLimitExceeded)?;
            CallBindingFault::MissingArguments {
                parameters: parameters.map(export_parameter_evidence).collect(),
                paramspec: paramspec
                    .map(|typevar| builder.intern(Type::TypeVar(typevar)))
                    .transpose()?,
            }
        }
        BindingError::UnknownArgument {
            argument_name,
            argument_index,
        } => CallBindingFault::UnknownArgument {
            name: argument_name.to_string(),
            argument: *argument_index,
        },
        BindingError::UnknownKeywordVariadicArgument { argument_index } => {
            CallBindingFault::UnknownKeywordVariadicArgument {
                argument: *argument_index,
            }
        }
        BindingError::PositionalOnlyParameterAsKwarg {
            argument_index,
            parameter,
        } => CallBindingFault::PositionalOnlyAsKeyword {
            argument: *argument_index,
            parameter: export_parameter_evidence(parameter.evidence()),
        },
        BindingError::TooManyPositionalArguments {
            first_excess_argument_index,
            expected_positional_count,
            provided_positional_count,
        } => CallBindingFault::TooManyPositionalArguments {
            first_excess: *first_excess_argument_index,
            expected: *expected_positional_count,
            provided: *provided_positional_count,
        },
        BindingError::ParameterAlreadyAssigned {
            argument_index,
            parameter,
        } => CallBindingFault::ParameterAlreadyAssigned {
            argument: *argument_index,
            parameter: export_parameter_evidence(parameter.evidence()),
        },
        BindingError::SpecializationError { argument_index, error } => match error {
            super::generics::SpecializationError::MismatchedBound {
                argument,
                bound_typevar,
            } => CallBindingFault::SpecializationBound {
                argument: *argument_index,
                provided: builder.intern(*argument)?,
                typevar: builder.intern(Type::TypeVar(*bound_typevar))?,
            },
            super::generics::SpecializationError::MismatchedConstraint {
                argument,
                bound_typevar,
            } => CallBindingFault::SpecializationConstraint {
                argument: *argument_index,
                provided: builder.intern(*argument)?,
                typevar: builder.intern(Type::TypeVar(*bound_typevar))?,
            },
        },
        BindingError::PropertyHasNoGetter(property) => CallBindingFault::PropertyGetterMissing {
            property: builder.intern(Type::PropertyInstance(*property))?,
        },
        BindingError::PropertyHasNoSetter(property) => CallBindingFault::PropertySetterMissing {
            property: builder.intern(Type::PropertyInstance(*property))?,
        },
        BindingError::PropertyHasNoDeleter(property) => CallBindingFault::PropertyDeleterMissing {
            property: builder.intern(Type::PropertyInstance(*property))?,
        },
        BindingError::PropertyGetterCallError(source) => CallBindingFault::PropertyGetterCall {
            source: Box::new(export_accessor_evidence(builder, source, depth + 1)?),
        },
        BindingError::PropertySetterCallError(source) => CallBindingFault::PropertySetterCall {
            source: Box::new(export_accessor_evidence(builder, source, depth + 1)?),
        },
        BindingError::InternalCallError(operation) => CallBindingFault::InternalCall { operation },
        BindingError::UnmatchedOverload => CallBindingFault::UnmatchedOverload,
        BindingError::CalledTopCallable(callable) => CallBindingFault::TopCallable {
            callable: builder.intern(*callable)?,
        },
        BindingError::InvalidDataclassApplication(target) => match target {
            super::call::bind::InvalidDataclassTarget::NamedTuple => CallBindingFault::DataclassNamedTuple,
            super::call::bind::InvalidDataclassTarget::TypedDict => CallBindingFault::DataclassTypedDict,
            super::call::bind::InvalidDataclassTarget::Enum => CallBindingFault::DataclassEnum,
            super::call::bind::InvalidDataclassTarget::Protocol => CallBindingFault::DataclassProtocol,
        },
        BindingError::InvalidDataclassArgument(argument) => match argument {
            super::call::bind::InvalidDataclassArgument::OrderRequiresEq => CallBindingFault::DataclassOrderRequiresEq,
            super::call::bind::InvalidDataclassArgument::WeakrefSlotRequiresSlots => {
                CallBindingFault::DataclassWeakrefSlotRequiresSlots
            }
        },
    })
}

fn export_accessor_evidence<'db>(
    builder: &mut Builder<'_, 'db>,
    source: &super::call::bind::PropertyAccessorCallError<'db>,
    depth: usize,
) -> Result<PropertyAccessorEvidence, ExportLimitExceeded> {
    let bindings = source.evidence_bindings();
    let callable = builder.intern(bindings.callable_type())?;
    let overloads = bindings
        .iter_flat()
        .map(|group| {
            group
                .into_iter()
                .map(|binding| {
                    binding
                        .errors()
                        .iter()
                        .map(|fault| export_binding_fault(builder, fault, depth))
                        .collect()
                })
                .collect()
        })
        .collect::<Result<_, ExportLimitExceeded>>()?;
    Ok(PropertyAccessorEvidence {
        callable,
        argument_index_offset: source.evidence_argument_offset(),
        overloads,
    })
}

pub(crate) fn rejected_call<'a, 'db: 'a>(
    db: &'db dyn Db,
    env: &ProgramEnvironment<'db>,
    faults: impl IntoIterator<Item = &'a super::call::bind::BindingError<'db>>,
) -> ArgumentBindingError {
    let mut builder = Builder {
        db,
        env,
        limit: 4096,
        references: 0,
        ids: HashMap::new(),
        pending: Vec::new(),
    };
    let result: Result<ArgumentBindingError, ExportLimitExceeded> = (|| {
        let faults = faults
            .into_iter()
            .map(|fault| export_binding_fault(&mut builder, fault, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let types = builder.finish_rejection_graph()?;
        Ok(ArgumentBindingError::CallRejected { faults, types })
    })();
    result.unwrap_or_else(|source| ArgumentBindingError::EvidenceLimit { source })
}

#[cfg(test)]
mod rejection_type_evidence_tests {
    use super::*;
    use crate::place::global_symbol;
    use ruff_db::files::system_path_to_file;
    use ruff_db::system::DbWithWritableSystem;
    use ty_python_core::ProgramFile;

    #[test]
    fn rejection_only_nodes_preserve_signatures_property_sharing_and_typevar_bounds() {
        let mut db = crate::db::tests::setup_db();
        db.write_dedented(
            "/src/rejection.py",
            r#"
def bounded[T: int](value: T, count: int = 1, /, *, flag: bool = False) -> T:
    return value
def constrained[U: (int, str)](value: U) -> U:
    return value
"#,
        )
        .unwrap();
        let env = db.program_environment();
        let file = system_path_to_file(&db, "/src/rejection.py").unwrap();
        let file = ProgramFile::new(&db, file, env.program(&db));
        let function_ty = global_symbol(&db, file, "bounded").place.expect_type();
        let Type::FunctionLiteral(function) = function_ty else {
            panic!("expected function literal");
        };
        let typevar = function
            .signature(&db)
            .iter()
            .next()
            .unwrap()
            .generic_context
            .unwrap()
            .variables(&db)
            .next()
            .unwrap();
        let callable =
            super::super::callable::CallableType::new(&db, function.signature(&db).clone(), CallableTypeKind::Regular);
        let property = super::super::PropertyInstanceType::new(&db, Some(function_ty), Some(function_ty), None);
        let roots = [
            function_ty,
            Type::Callable(callable),
            Type::PropertyInstance(property),
            Type::TypeVar(typevar),
            global_symbol(&db, file, "constrained").place.expect_type(),
        ];
        // Diagnostic support must not expand the materialized inference contract.
        assert!(
            export(&db, &env, &roots, 4096)
                .unwrap()
                .nodes
                .iter()
                .all(|node| matches!(node, Node::Unsupported))
        );
        let mut builder = Builder {
            db: &db,
            env: &env,
            limit: 4096,
            references: 0,
            ids: HashMap::new(),
            pending: Vec::new(),
        };
        let ids = builder.ids(&roots).unwrap();
        let graph = builder.finish_rejection_graph().unwrap();
        let RejectionTypeNode::FunctionLiteral { identity, signatures } = &graph.nodes[ids[0].0 as usize] else {
            panic!("function collapsed");
        };
        assert_eq!(identity.source, "/src/rejection.py");
        assert_eq!(identity.path, vec!["bounded"]);
        let signature = &signatures[0];
        assert_eq!(signature.parameters[0].name.as_deref(), Some("value"));
        assert_eq!(signature.parameters[0].kind, RejectionParameterKind::PositionalOnly);
        assert_eq!(signature.parameters[0].annotation, ids[3]);
        assert_eq!(signature.return_type, ids[3]);
        assert!(matches!(
            graph.nodes[signature.parameters[1].default.unwrap().0 as usize],
            RejectionTypeNode::Value(Node::IntLiteral(1))
        ));
        assert!(
            matches!(&graph.nodes[ids[1].0 as usize], RejectionTypeNode::Callable { kind: CallableTypeKind::Regular, signatures } if !signatures.is_empty())
        );
        assert!(
            matches!(&graph.nodes[ids[2].0 as usize], RejectionTypeNode::PropertyInstance { class: RejectionPropertyClass::Builtin, getter: Some(getter), setter: Some(setter), deleter: None } if *getter == ids[0] && *setter == ids[0])
        );
        assert!(
            matches!(&graph.nodes[ids[3].0 as usize], RejectionTypeNode::TypeVar { name, kind: TypeVarKind::Pep695TypeVar, freshness: 0, bound_or_constraints: Some(RejectionTypeVarBounds::UpperBound(_)), .. } if name == "T")
        );
        assert!(graph.nodes.iter().any(|node| matches!(node, RejectionTypeNode::TypeVar { name, bound_or_constraints: Some(RejectionTypeVarBounds::Constraints(items)), .. } if name == "U" && items.len() == 2)));
    }
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
