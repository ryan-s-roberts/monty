#![doc = include_str!("../README.md")]

use monty_type_checking::{SourceFile, TypeChecker};
pub use monty_types::analysis::*;
use ruff_db::{files::File, parsed::parsed_module};
use ruff_python_ast::{
    Expr, Stmt,
    visitor::{self, Visitor},
};
use ruff_text_size::Ranged;
use ty_python_semantic::{Db, HasType, ProgramEnvironment, SemanticModel, types::export};
mod branches;
mod graph;
pub use branches::{Binding, BranchRequest, analyze_branches};

/// Analyze a fresh source unit without evaluating Python or calling host functions.
/// Each call owns a fresh checker, including on errors and unwinding.
/// This is synchronous compiler work; callers own CPU scheduling.
pub fn analyze(request: &AnalysisRequest) -> Result<AnalysisResult, String> {
    validate(request)?;
    let mut checker = TypeChecker::default();
    let source = SourceFile::new(&request.source, "analysis.py");
    let stubs = request
        .stubs
        .as_deref()
        .map(|s| SourceFile::new(s, "analysis_stubs.pyi"));
    let result = checker.inspect(&source, stubs.as_ref(), |db, file, offset| {
        infer(db, file, offset, request)
    })?;
    let outcome = match result {
        Ok(graph) => AnalysisOutcome::Inferred(graph?),
        Err(diagnostics) => AnalysisOutcome::Rejected(diagnostics),
    };
    Ok(AnalysisResult { outcome })
}

fn validate(request: &AnalysisRequest) -> Result<(), String> {
    let bytes = request
        .source
        .len()
        .saturating_add(request.stubs.as_ref().map_or(0, String::len));
    if bytes > request.limits.max_source_bytes.min(1_048_576) as usize {
        return Err("analysis source byte limit exceeded".into());
    }
    if request.targets.len() > request.limits.max_targets.min(1024) as usize {
        return Err("analysis target limit exceeded".into());
    }
    if request.limits.max_type_references == 0 || request.limits.max_type_references > 65_536 {
        return Err("analysis type reference limit must be in 1..=65536".into());
    }
    for target in &request.targets {
        if target.start >= target.end
            || target.end as usize > request.source.len()
            || !request.source.is_char_boundary(target.start as usize)
            || !request.source.is_char_boundary(target.end as usize)
        {
            return Err("analysis target is not a valid nonempty UTF-8 source range".into());
        }
    }
    Ok(())
}

fn infer(db: &dyn Db, file: File, offset: u32, request: &AnalysisRequest) -> Result<Graph, String> {
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
        return Err("analysis AST nesting limit exceeded".into());
    }
    let model = SemanticModel::new(db, program_file);
    let types = selector
        .found
        .into_iter()
        .map(|expr| {
            expr.ok_or_else(|| "analysis target must match an expression exactly".to_string())?
                .inferred_type(&model)
                .ok_or_else(|| "expression has no inferred type".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let graph = export::export(
        db,
        &ProgramEnvironment::from_file(program_file),
        &types,
        request.limits.max_type_references,
    )
    .map_err(|_| "analysis type graph limit exceeded".to_string())?;
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
