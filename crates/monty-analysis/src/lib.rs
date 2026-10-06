#![doc = include_str!("../README.md")]

use monty_type_checking::{SourceFile, TypeChecker};
pub use monty_types::analysis::*;
use ruff_db::{files::File, parsed::parsed_module};
use ruff_python_ast::{
    Expr, Stmt,
    visitor::{self, Visitor},
};
use ruff_text_size::Ranged;
use std::sync::Arc;
use thiserror::Error;
use ty_python_semantic::{Db, HasType, ProgramEnvironment, SemanticModel, types::export};
mod branches;
mod control_flow;
pub use control_flow::{FlowExit, FunctionFlow, function_flow};
mod graph;
pub use branches::{Binding, BranchRequest, analyze_branches};

#[derive(Debug, Clone, Error)]
pub enum AnalysisError {
    #[error("analysis source byte limit exceeded")]
    SourceByteLimitExceeded,
    #[error("analysis target limit exceeded")]
    TargetLimitExceeded,
    #[error("analysis type reference limit must be in 1..=65536")]
    InvalidTypeReferenceLimit,
    #[error("analysis target is not a valid nonempty UTF-8 source range")]
    InvalidTargetSpan,
    #[error("analysis AST nesting limit exceeded")]
    AstNestingLimitExceeded,
    #[error("analysis target must match an expression exactly")]
    TargetExpressionMissing,
    #[error("expression has no inferred type")]
    InferredTypeMissing,
    #[error("analysis type graph limit exceeded")]
    TypeGraphLimitExceeded {
        #[source]
        source: export::ExportLimitExceeded,
    },
    #[error("analysis function span is not a valid nonempty UTF-8 source range")]
    InvalidFunctionSpan,
    #[error("analysis function is absent")]
    FunctionMissing,
    #[error("lexical target must match a function")]
    LexicalFunctionMissing,
    #[error("lexical AST nesting limit exceeded")]
    LexicalAstNestingLimitExceeded {
        #[source]
        source: export::ExportLimitExceeded,
    },
    #[error("callable definition is absent")]
    CallableDefinitionMissing,
    #[error("control-flow export limit exceeded")]
    ControlFlowExportLimitExceeded {
        #[source]
        source: export::ExportLimitExceeded,
    },
    #[error("control-flow projection budget exceeded")]
    ControlFlowProjectionBudgetExceeded,
    #[error("branch analysis target limit exceeded")]
    BranchTargetLimitExceeded,
    #[error("branch analysis source byte limit exceeded")]
    BranchSourceByteLimitExceeded,
    #[error("branch binding requires an ASCII identifier")]
    InvalidBranchBinding,
    #[error("branch source is too large")]
    BranchSourceTooLarge,
    #[error("analysis checker bridge failed: {source}")]
    CheckerBridge {
        #[source]
        source: Arc<monty_type_checking::TypeCheckerError>,
    },
    #[error("analysis argument binding failed: {source}")]
    ArgumentBinding {
        #[source]
        source: export::ArgumentBindingError,
    },
}

/// Analyze a fresh source unit without evaluating Python or calling host functions.
/// Each call owns a fresh checker, including on errors and unwinding.
/// This is synchronous compiler work; callers own CPU scheduling.
pub fn analyze(request: &AnalysisRequest) -> Result<AnalysisResult, AnalysisError> {
    validate(request)?;
    let mut checker = TypeChecker::default();
    let source = SourceFile::new(&request.source, "analysis.py");
    let stubs = request
        .stubs
        .as_deref()
        .map(|s| SourceFile::new(s, "analysis_stubs.pyi"));
    let result = checker
        .inspect(&source, stubs.as_ref(), |db, file, offset| {
            infer(db, file, offset, request)
        })
        .map_err(|source| AnalysisError::CheckerBridge {
            source: Arc::new(source),
        })?;
    let outcome = match result {
        Ok(graph) => AnalysisOutcome::Inferred(graph?),
        Err(diagnostics) => AnalysisOutcome::Rejected(diagnostics),
    };
    Ok(AnalysisResult { outcome })
}

fn validate(request: &AnalysisRequest) -> Result<(), AnalysisError> {
    let bytes = request
        .source
        .len()
        .saturating_add(request.stubs.as_ref().map_or(0, String::len));
    if bytes > request.limits.max_source_bytes.min(1_048_576) as usize {
        return Err(AnalysisError::SourceByteLimitExceeded);
    }
    if request.targets.len() > request.limits.max_targets.min(1024) as usize {
        return Err(AnalysisError::TargetLimitExceeded);
    }
    if request.limits.max_type_references == 0 || request.limits.max_type_references > 65_536 {
        return Err(AnalysisError::InvalidTypeReferenceLimit);
    }
    for target in &request.targets {
        if target.start >= target.end
            || target.end as usize > request.source.len()
            || !request.source.is_char_boundary(target.start as usize)
            || !request.source.is_char_boundary(target.end as usize)
        {
            return Err(AnalysisError::InvalidTargetSpan);
        }
    }
    Ok(())
}

fn infer(db: &dyn Db, file: File, offset: u32, request: &AnalysisRequest) -> Result<Graph, AnalysisError> {
    let program_file = db.program_file(file);
    let parsed = parsed_module(db, program_file.python_file(db)).load(db);
    let mut selector = Selector {
        targets: &request.targets,
        offset,
        found: vec![None; request.targets.len()],
        depth: 0,
        too_deep: false,
    };
    selector.visit_body(parsed.suite());
    if selector.too_deep {
        return Err(AnalysisError::AstNestingLimitExceeded);
    }
    let model = SemanticModel::new(db, program_file);
    let types = selector
        .found
        .into_iter()
        .map(|expr| {
            expr.ok_or(AnalysisError::TargetExpressionMissing)?
                .inferred_type(&model)
                .ok_or(AnalysisError::InferredTypeMissing)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let graph = export::export(
        db,
        &ProgramEnvironment::from_file(program_file),
        &types,
        request.limits.max_type_references,
    )
    .map_err(|source| AnalysisError::TypeGraphLimitExceeded { source })?;
    Ok(graph::convert(graph, offset))
}

struct Selector<'a, 'ast> {
    targets: &'a [Span],
    offset: u32,
    found: Vec<Option<&'ast Expr>>,
    depth: usize,
    too_deep: bool,
}
impl<'ast> Visitor<'ast> for Selector<'_, 'ast> {
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if self.depth >= 128 {
            self.too_deep = true;
            return;
        }
        self.depth += 1;
        visitor::walk_stmt(self, stmt);
        self.depth -= 1;
    }
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if self.depth >= 128 {
            self.too_deep = true;
            return;
        }
        for (index, target) in self.targets.iter().enumerate() {
            if expr.range().start().to_u32() == target.start + self.offset
                && expr.range().end().to_u32() == target.end + self.offset
            {
                self.found[index] = Some(expr);
            }
        }
        self.depth += 1;
        visitor::walk_expr(self, expr);
        self.depth -= 1;
    }
}

/// Analyze a named top-level function's complete return boundary. Python's
/// checker owns local flow, container inference and implicit-return reachability.
pub fn analyze_function(request: &AnalysisRequest, name: &str) -> Result<AnalysisResult, AnalysisError> {
    analyze_selected_function(request, FunctionSelector::TopLevelName(name))
}

/// Analyze a function at its original source span, including a lexically nested helper.
/// The checker still owns body flow and return types; the span only selects its AST node.
pub fn analyze_function_at(request: &AnalysisRequest, span: Span) -> Result<AnalysisResult, AnalysisError> {
    if span.start >= span.end || request.source.get(span.start as usize..span.end as usize).is_none() {
        return Err(AnalysisError::InvalidFunctionSpan);
    }
    analyze_selected_function(request, FunctionSelector::Span(span))
}

enum FunctionSelector<'a> {
    TopLevelName(&'a str),
    Span(Span),
}

fn analyze_selected_function(
    request: &AnalysisRequest,
    selector: FunctionSelector<'_>,
) -> Result<AnalysisResult, AnalysisError> {
    validate(request)?;
    let mut checker = TypeChecker::default();
    let source = SourceFile::new(&request.source, "analysis.py");
    let stubs = request
        .stubs
        .as_deref()
        .map(|s| SourceFile::new(s, "analysis_stubs.pyi"));
    let result = checker
        .inspect(&source, stubs.as_ref(), |db, file, offset| {
            let program_file = db.program_file(file);
            let parsed = parsed_module(db, program_file.python_file(db)).load(db);
            let function = match selector {
                FunctionSelector::TopLevelName(name) => parsed.suite().iter().find_map(|stmt| match stmt {
                    Stmt::FunctionDef(function) if function.name.as_str() == name => Some(function),
                    _ => None,
                }),
                FunctionSelector::Span(span) => {
                    struct Find<'a> {
                        span: Span,
                        found: Option<&'a ruff_python_ast::StmtFunctionDef>,
                    }
                    impl<'a> Visitor<'a> for Find<'a> {
                        fn visit_stmt(&mut self, stmt: &'a Stmt) {
                            if let Stmt::FunctionDef(function) = stmt {
                                if function.start().to_u32() == self.span.start
                                    && function.end().to_u32() == self.span.end
                                {
                                    self.found = Some(function);
                                    return;
                                }
                            }
                            visitor::walk_stmt(self, stmt);
                        }
                    }
                    let mut find = Find {
                        span: Span {
                            start: span.start + offset,
                            end: span.end + offset,
                        },
                        found: None,
                    };
                    find.visit_body(parsed.suite());
                    find.found
                }
            }
            .ok_or(AnalysisError::FunctionMissing)?;
            export::function_returns(db, file, function, request.limits.max_type_references)
                .map(|graph| graph::convert(graph, offset))
                .map_err(|source| AnalysisError::TypeGraphLimitExceeded { source })
        })
        .map_err(|source| AnalysisError::CheckerBridge {
            source: Arc::new(source),
        })?;
    Ok(AnalysisResult {
        outcome: match result {
            Ok(graph) => AnalysisOutcome::Inferred(graph?),
            Err(errors) => AnalysisOutcome::Rejected(errors),
        },
    })
}

/// Lexical facts only. This does not claim type or runtime admission.
pub fn function_locals(source: &str, target: Span) -> Result<Vec<String>, AnalysisError> {
    let request = AnalysisRequest {
        source: source.into(),
        stubs: None,
        targets: vec![target],
        limits: AnalysisLimits::default(),
    };
    validate(&request)?;
    let mut checker = TypeChecker::default();
    checker
        .inspect_syntax(&SourceFile::new(source, "scope.py"), |db, file| {
            struct Find<'a> {
                target: Span,
                found: Option<&'a ruff_python_ast::StmtFunctionDef>,
                depth: usize,
            }
            impl<'a> Visitor<'a> for Find<'a> {
                fn visit_stmt(&mut self, stmt: &'a Stmt) {
                    if self.depth >= 128 {
                        return;
                    }
                    if let Stmt::FunctionDef(function) = stmt {
                        if function.start().to_u32() == self.target.start && function.end().to_u32() == self.target.end
                        {
                            self.found = Some(function);
                            return;
                        }
                    }
                    self.depth += 1;
                    visitor::walk_stmt(self, stmt);
                    self.depth -= 1;
                }
            }
            let parsed = parsed_module(db, db.program_file(file).python_file(db)).load(db);
            let mut find = Find {
                target,
                found: None,
                depth: 0,
            };
            find.visit_body(parsed.suite());
            let function = find.found.ok_or(AnalysisError::LexicalFunctionMissing)?;
            Ok(export::function_locals(db, file, function))
        })
        .map_err(|source| AnalysisError::CheckerBridge {
            source: Arc::new(source),
        })?
}

/// Lexical external loads, with source coordinates. No type admission implied.
pub fn external_names(source: &str) -> Result<Vec<(Span, String)>, AnalysisError> {
    let request = AnalysisRequest {
        source: source.into(),
        stubs: None,
        targets: vec![],
        limits: AnalysisLimits::default(),
    };
    validate(&request)?;
    let mut checker = TypeChecker::default();
    checker
        .inspect_syntax(&SourceFile::new(source, "captures.py"), |db, file| {
            export::external_names(db, file)
                .map(|names| {
                    names
                        .into_iter()
                        .map(|(start, end, name)| (Span { start, end }, name))
                        .collect()
                })
                .map_err(|source| AnalysisError::LexicalAstNestingLimitExceeded { source })
        })
        .map_err(|source| AnalysisError::CheckerBridge {
            source: Arc::new(source),
        })?
}

pub use export::ArgumentBinding;

/// Bind a host-supplied row using the upstream Python call binder. This API is
/// lexical/arity evidence only; argument and result contracts are checked later.
pub fn bind_one_positional(source: &str) -> Result<ArgumentBinding, AnalysisError> {
    inspect_function(source, |db, file, function| {
        export::bind_one_positional(db, file, function).map_err(|source| AnalysisError::ArgumentBinding { source })
    })
}

/// Resolve explicit call arguments through the Python call binder, in source order.
pub fn bind_arguments(source: &str, arguments: &[Option<String>]) -> Result<Vec<ArgumentBinding>, AnalysisError> {
    inspect_function(source, |db, file, function| {
        export::bind_arguments(db, file, function, arguments)
            .map_err(|source| AnalysisError::ArgumentBinding { source })
    })
}

/// Reachable statement spans and implicit-return evidence for one function.
pub fn reachable_statements(source: &str) -> Result<(Vec<Span>, bool), AnalysisError> {
    inspect_function(source, |db, file, function| {
        export::reachable_statements(db, file, function)
            .map(|(ranges, fallthrough)| {
                (
                    ranges.into_iter().map(|(start, end)| Span { start, end }).collect(),
                    fallthrough,
                )
            })
            .map_err(|source| AnalysisError::ControlFlowExportLimitExceeded { source })
    })
}

fn inspect_function<T>(
    source: &str,
    f: impl FnOnce(&dyn Db, File, &ruff_python_ast::StmtFunctionDef) -> Result<T, AnalysisError>,
) -> Result<T, AnalysisError> {
    inspect_function_at(source, None, f)
}

fn inspect_function_at<T>(
    source: &str,
    target: Option<Span>,
    f: impl FnOnce(&dyn Db, File, &ruff_python_ast::StmtFunctionDef) -> Result<T, AnalysisError>,
) -> Result<T, AnalysisError> {
    validate(&AnalysisRequest {
        source: source.into(),
        stubs: None,
        targets: target.into_iter().collect(),
        limits: AnalysisLimits::default(),
    })?;
    TypeChecker::default()
        .inspect_syntax(&SourceFile::new(source, "callable.py"), |db, file| {
            let parsed = parsed_module(db, db.program_file(file).python_file(db)).load(db);
            struct Find<'a> {
                target: Option<Span>,
                found: Option<&'a ruff_python_ast::StmtFunctionDef>,
                depth: usize,
            }
            impl<'a> Visitor<'a> for Find<'a> {
                fn visit_stmt(&mut self, stmt: &'a Stmt) {
                    if self.depth >= 128 {
                        return;
                    }
                    if let Stmt::FunctionDef(function) = stmt {
                        if self
                            .target
                            .is_none_or(|s| s.start == function.start().to_u32() && s.end == function.end().to_u32())
                        {
                            self.found = Some(function);
                            return;
                        }
                    }
                    self.depth += 1;
                    visitor::walk_stmt(self, stmt);
                    self.depth -= 1;
                }
            }
            let mut find = Find {
                target,
                found: None,
                depth: 0,
            };
            find.visit_body(parsed.suite());
            f(db, file, find.found.ok_or(AnalysisError::CallableDefinitionMissing)?)
        })
        .map_err(|source| AnalysisError::CheckerBridge {
            source: Arc::new(source),
        })?
}

#[cfg(test)]
mod callable_evidence_tests {
    use super::*;
    #[test]
    fn lexical_syntax_preserves_checker_and_parser_causes() {
        use std::error::Error;
        let error = bind_one_positional("def callback(:\n    pass\n").unwrap_err();
        let AnalysisError::CheckerBridge { source } = &error else {
            panic!("expected checker bridge fault: {error:?}");
        };
        assert!(matches!(
            source.as_ref(),
            monty_type_checking::TypeCheckerError::LexicalSyntax { .. }
        ));
        assert!(source.source().unwrap().downcast_ref::<export::ParseError>().is_some());
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<Arc<monty_type_checking::TypeCheckerError>>()
                .is_some()
        );
    }

    #[test]
    fn rejected_argument_binding_has_owned_semantic_evidence() {
        use std::error::Error;
        let error = bind_one_positional("def callback(row, required):\n    pass\n").unwrap_err();
        assert!(matches!(&error, AnalysisError::ArgumentBinding {
            source: export::ArgumentBindingError::CallRejected { faults, .. }
        } if faults.iter().any(|fault| matches!(fault, export::CallBindingFault::MissingArguments { .. }))));
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<export::ArgumentBindingError>()
                .is_some()
        );
    }
    #[test]
    fn call_binding_uses_python_required_default_and_parameter_kinds() {
        for signature in [
            "row",
            "row, /",
            "row=None",
            "row, extra=1",
            "row, *, extra=1",
            "row, *args, **kwargs",
        ] {
            let binding = bind_one_positional(&format!("def callback({signature}):\n    pass\n")).unwrap();
            assert_eq!(binding.parameter, "row");
            assert!(!binding.variadic);
        }
        for signature in ["", "*, row", "row, required", "row, *, required"] {
            assert!(
                bind_one_positional(&format!("def callback({signature}):\n    pass\n")).is_err(),
                "{signature}"
            );
        }
        assert!(
            bind_one_positional("def callback(*rows):\n    pass\n")
                .unwrap()
                .variadic
        );
    }
    #[test]
    fn expression_flow_retains_the_original_conditional() {
        let source = "def callback(row):\n    return row.value if row.active else None\n";
        let flow = function_flow(
            source,
            Span {
                start: 0,
                end: source.trim_end().len() as u32,
            },
        )
        .unwrap();
        let FlowExit::Branch {
            source_expression: Some(ruff_python_ast::Expr::If(_)),
            ..
        } = flow.exit
        else {
            panic!("return expression must remain available intact")
        };
    }
    #[test]
    fn flow_excludes_dead_writes_and_preserves_implicit_returns() {
        let source = "def callback(row):\n    if False:\n        row.bad()\n    row.good()\n";
        let flow = function_flow(
            source,
            Span {
                start: 0,
                end: source.trim_end().len() as u32,
            },
        )
        .unwrap();
        assert!(flow.returns_only_none());
        let FlowExit::Branch { selected, no, .. } = flow.exit else {
            panic!("condition evaluation must be preserved")
        };
        assert_eq!(selected, Some(false));
        assert_eq!(no.statements.len(), 1);
        let source = "def callback(row):\n    if row.active:\n        return row.value\n    row.good()\n";
        let flow = function_flow(
            source,
            Span {
                start: 0,
                end: source.trim_end().len() as u32,
            },
        )
        .unwrap();
        let FlowExit::Branch { yes, no, .. } = flow.exit else {
            panic!("expected conditional flow")
        };
        assert!(!yes.returns_only_none());
        assert!(no.returns_only_none());
        assert_eq!(no.statements.len(), 1);
    }
}

#[cfg(test)]
mod argument_binding_tests {
    use super::*;
    #[test]
    fn missing_parameter_evidence_obeys_the_export_budget() {
        use ty_python_semantic::types::export::ArgumentBindingError;

        for (count, exceeds_budget) in [(4095, false), (4096, true)] {
            let parameters = (0..count)
                .map(|index| format!("p{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let source = format!("def f({parameters}):\n    pass\n");
            let error = bind_arguments(&source, &[]).unwrap_err();
            match error {
                AnalysisError::ArgumentBinding {
                    source: ArgumentBindingError::CallRejected { .. },
                } if !exceeds_budget => {}
                AnalysisError::ArgumentBinding {
                    source: ArgumentBindingError::EvidenceLimit { .. },
                } if exceeds_budget => {}
                other => panic!("unexpected evidence outcome for {count} missing parameters: {other:?}"),
            }
        }
    }

    #[test]
    fn upstream_call_binding_preserves_argument_order_and_parameter_kinds() {
        let source = "def f(first, /, second, *, third=3):\n    pass\n";
        let result = bind_arguments(source, &[None, Some("third".into()), Some("second".into())]).unwrap();
        assert_eq!(
            result.iter().map(|b| b.parameter.as_str()).collect::<Vec<_>>(),
            ["first", "third", "second"]
        );
        assert!(bind_arguments(source, &[Some("first".into()), Some("second".into())]).is_err());
        assert!(bind_arguments(source, &[None, None, Some("second".into())]).is_err());
        assert!(bind_arguments(source, &[None]).is_err());
        assert!(bind_arguments(source, &[None, None]).is_ok());
    }

    #[test]
    fn rejected_calls_retain_owned_parameter_evidence_and_typed_source() {
        use std::error::Error;
        use ty_python_semantic::types::export::{ArgumentBindingError, CallBindingFault};

        let source = "def f(first, /, second, *, third=3):\n    pass\n";
        let error = bind_arguments(source, &[None]).unwrap_err();
        assert!(error.source().unwrap().is::<ArgumentBindingError>());
        let AnalysisError::ArgumentBinding {
            source: ArgumentBindingError::CallRejected { faults, types },
        } = error
        else {
            panic!("expected typed call rejection: {error:?}");
        };
        assert!(
            faults.iter().any(|fault| matches!(
                fault,
                CallBindingFault::MissingArguments { parameters, .. }
                    if parameters.iter().any(|parameter|
                        parameter.name.as_ref().is_some_and(|name| name.name == "second")
                            && parameter.signature_parameter_index == 1
                            && parameter.source_parameter_index == Some(1)
                            && !parameter.positional)
            )),
            "{faults:?}"
        );
        assert!(types.roots.is_empty());
        assert!(types.nodes.is_empty());

        let error = bind_arguments(source, &[None, None, Some("second".into())]).unwrap_err();
        let AnalysisError::ArgumentBinding {
            source: ArgumentBindingError::CallRejected { faults, .. },
        } = error
        else {
            panic!("expected typed duplicate assignment: {error:?}");
        };
        assert!(
            faults.iter().any(|fault| matches!(
                fault,
                CallBindingFault::ParameterAlreadyAssigned { parameter, argument: Some(2) }
                    if parameter.name.as_ref().is_some_and(|name| name.name == "second")
                        && parameter.signature_parameter_index == 1
            )),
            "{faults:?}"
        );
    }
}
