//! Non-executing analysis boundary. All offsets are UTF-8 byte offsets.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Half-open UTF-8 byte range in the original source, before stub injection.
pub struct Span {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisRequest {
    pub source: String,
    pub stubs: Option<String>,
    pub targets: Vec<Span>,
    pub limits: AnalysisLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisLimits {
    /// Combined source/stub bytes; additionally capped at 1 MiB.
    pub max_source_bytes: u32,
    /// Requested expression roots; additionally capped at 1024.
    pub max_targets: u32,
    /// Root and edge references, including repeated references; must be 1..=65536.
    pub max_type_references: u32,
}
impl Default for AnalysisLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 262_144,
            max_targets: 256,
            max_type_references: 16_384,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisDiagnostic {
    pub code: String,
    pub message: String,
    pub source: Option<String>,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisOutcome {
    /// Roots correspond one-for-one to request targets, in request order.
    Inferred(Graph),
    Rejected(Vec<AnalysisDiagnostic>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisResult {
    pub outcome: AnalysisOutcome,
}

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
