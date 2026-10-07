use monty_types::analysis::{Field, Graph, Identity, Node, Openness, TypeId};
use ty_python_semantic::types::export as ty;

pub(crate) fn convert(graph: ty::Graph, offset: u32) -> Graph {
    let mut graph = Graph {
        roots: ids(graph.roots),
        nodes: graph.nodes.into_iter().map(node).collect(),
    };
    for node in &mut graph.nodes {
        let identity = match node {
            Node::Protocol { identity, .. }
            | Node::Instance { identity, .. }
            | Node::NewType { identity, .. }
            | Node::EnumLiteral { identity, .. } => Some(identity),
            Node::Record { identity, .. } => identity.as_mut(),
            _ => None,
        };
        if let Some(identity) = identity
            && identity.source == "/analysis.py"
        {
            identity.start = identity.start.and_then(|start| start.checked_sub(offset));
        }
    }
    graph
}
fn ids(ids: Vec<ty::TypeId>) -> Vec<TypeId> {
    ids.into_iter().map(|id| TypeId(id.0)).collect()
}
fn identity(id: ty::Identity) -> Identity {
    Identity {
        source: id.source,
        start: id.start,
        path: id.path,
    }
}
fn node(node: ty::Node) -> Node {
    match node {
        ty::Node::Any => Node::Any,
        ty::Node::Unknown => Node::Unknown,
        ty::Node::Unsupported => Node::Unsupported,
        ty::Node::Never => Node::Never,
        ty::Node::None => Node::None,
        ty::Node::BoolLiteral(v) => Node::BoolLiteral(v),
        ty::Node::IntLiteral(v) => Node::IntLiteral(v),
        ty::Node::StringLiteral(v) => Node::StringLiteral(v),
        ty::Node::BytesLiteral(v) => Node::BytesLiteral(v),
        ty::Node::LiteralString => Node::LiteralString,
        ty::Node::EnumLiteral { identity: id, member } => Node::EnumLiteral {
            identity: identity(id),
            member,
        },
        ty::Node::Instance {
            identity: id,
            arguments,
        } => Node::Instance {
            identity: identity(id),
            arguments: ids(arguments),
        },
        ty::Node::Protocol {
            identity: id,
            arguments,
        } => Node::Protocol {
            identity: identity(id),
            arguments: ids(arguments),
        },
        ty::Node::Tuple {
            prefix,
            variable,
            suffix,
        } => Node::Tuple {
            prefix: ids(prefix),
            variable: variable.map(|id| TypeId(id.0)),
            suffix: ids(suffix),
        },
        ty::Node::Union(items) => Node::Union(ids(items)),
        ty::Node::Intersection { positive, negative } => Node::Intersection {
            positive: ids(positive),
            negative: ids(negative),
        },
        ty::Node::Truthy => Node::Truthy,
        ty::Node::Falsy => Node::Falsy,
        ty::Node::Record {
            identity: id,
            fields,
            openness,
        } => Node::Record {
            identity: id.map(identity),
            fields: fields
                .into_iter()
                .map(|f| Field {
                    name: f.name,
                    ty: TypeId(f.ty.0),
                    required: f.required,
                    read_only: f.read_only,
                })
                .collect(),
            openness: match openness {
                ty::Openness::Implicit => Openness::Implicit,
                ty::Openness::Closed => Openness::Closed,
                ty::Openness::Extra { ty, read_only } => Openness::Extra {
                    ty: TypeId(ty.0),
                    read_only,
                },
            },
        },
        ty::Node::NewType { identity: id, base } => Node::NewType {
            identity: identity(id),
            base: TypeId(base.0),
        },
        ty::Node::Alias(id) => Node::Alias(TypeId(id.0)),
    }
}
