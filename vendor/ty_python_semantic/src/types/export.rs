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
