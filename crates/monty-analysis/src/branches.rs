//! Branch evidence is inferred by the Python checker, never by matching predicates.
use crate::{AnalysisLimits, AnalysisRequest, AnalysisResult, Span, analyze};

/// A typed lexical binding in the expression's environment.
#[derive(Debug, Clone)]
pub struct Binding {
    pub name: String,
    pub annotation: String,
}

/// Analyze the subjects at both successors of a predicate. This does not execute
/// the predicate or subjects. Subjects are evaluated only by the static checker.
#[derive(Debug, Clone)]
pub struct BranchRequest {
    pub bindings: Vec<Binding>,
    pub predicate: String,
    pub subjects: Vec<String>,
    pub imports: String,
    pub stubs: Option<String>,
    pub limits: AnalysisLimits,
}

/// Graph roots are ordered: all true-branch subjects, then all false-branch
/// subjects, in request order. Never denotes an unreachable observation.
pub fn analyze_branches(request: &BranchRequest) -> Result<AnalysisResult, String> {
    let targets = request
        .subjects
        .len()
        .saturating_mul(2)
        .saturating_add(request.bindings.len())
        .saturating_add(1);
    if targets > request.limits.max_targets.min(1024) as usize {
        return Err("branch analysis target limit exceeded".into());
    }
    let bytes = request
        .bindings
        .iter()
        .fold(0usize, |n, b| {
            n.saturating_add(b.name.len()).saturating_add(b.annotation.len())
        })
        .saturating_add(request.predicate.len())
        .saturating_add(request.imports.len())
        .saturating_add(request.stubs.as_ref().map_or(0, String::len))
        .saturating_add(
            request
                .subjects
                .iter()
                .fold(0usize, |n, s| n.saturating_add(s.len().saturating_mul(2))),
        );
    if bytes > request.limits.max_source_bytes.min(1_048_576) as usize {
        return Err("branch analysis source byte limit exceeded".into());
    }
    let mut source = format!("{}\ndef __monty_branch(", request.imports);
    let mut targets = Vec::new();
    for (index, binding) in request.bindings.iter().enumerate() {
        let mut bytes = binding.name.bytes();
        if !bytes.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            || !bytes.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err("branch binding requires an ASCII identifier".into());
        }
        if index != 0 {
            source.push_str(", ");
        }
        source.push_str(&binding.name);
        source.push_str(": ");
        append_target(&mut source, &mut targets, &binding.annotation)?;
    }
    source.push_str("):\n    if (");
    append_target(&mut source, &mut targets, &request.predicate)?;
    source.push_str("):\n");
    let checked_prefix = targets.len();
    for branch in 0..2 {
        if branch == 1 {
            source.push_str("    else:\n");
        }
        source.push_str("        pass\n");
        for subject in &request.subjects {
            source.push_str("        (");
            let start = u32::try_from(source.len()).map_err(|_| "branch source is too large")?;
            source.push_str(subject);
            targets.push(Span {
                start,
                end: u32::try_from(source.len()).map_err(|_| "branch source is too large")?,
            });
            source.push_str(")\n");
        }
    }
    let mut result = analyze(&AnalysisRequest {
        source,
        stubs: request.stubs.clone(),
        targets,
        limits: request.limits,
    })?;
    if let crate::AnalysisOutcome::Inferred(graph) = &mut result.outcome {
        graph.roots.drain(..checked_prefix);
    }
    Ok(result)
}

fn append_target(source: &mut String, targets: &mut Vec<Span>, expression: &str) -> Result<(), String> {
    let start = u32::try_from(source.len()).map_err(|_| "branch source is too large")?;
    source.push_str(expression);
    targets.push(Span {
        start,
        end: u32::try_from(source.len()).map_err(|_| "branch source is too large")?,
    });
    Ok(())
}
