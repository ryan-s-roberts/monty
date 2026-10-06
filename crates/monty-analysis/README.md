# monty-analysis

Non-executing expression type analysis using Monty's checker and typeshed.
Source and optional stubs are isolated in a fresh in-memory checker per request.
Targets are exact UTF-8 byte ranges of expressions in the original source.
Results are owned graphs; roots follow target order.
Graphs preserve shared and recursive types, unions, literal values, nominal
identities, generic arguments, tuple shape, and TypedDict presence and openness.
Any, Unknown and Unsupported are distinct outcomes, never permission to execute.

This analyzes Python types, not runtime availability, effects, authorization,
cardinality, or whether all syntax is executable by Monty. Consumers enforce
those policies separately. Analysis never executes source or invokes callbacks.
Direct calls bound source size, targets and exported references. Analysis is a synchronous in-process call, independent of the worker pool.
Empty targets request definition checking only. Callers own CPU scheduling.
Source, AST depth and graph limits are admission bounds, not allocator or wall-clock limits.
`analyze_function_at` selects a top-level or nested function by its original
source span (before stub injection) and exports its checked reachable returns.

Development depends on the pinned ty structured export patch documented in
../../vendor/ty_python_semantic/MONTY-PATCH.md. A public release requires that
API to be available as a resolvable dependency. Nothing is published by this setup.


```rust
use monty_analysis::{analyze, AnalysisLimits, AnalysisRequest, AnalysisOutcome, Span};

let source = "name.lower()";
let result = analyze(&AnalysisRequest {
    source: source.into(),
    stubs: Some("name: str".into()),
    targets: vec![Span { start: 0, end: u32::try_from(source.len()).unwrap() }],
    limits: AnalysisLimits::default(),
})?;
assert!(matches!(result.outcome, AnalysisOutcome::Inferred(_)));
# Ok::<(), monty_analysis::AnalysisError>(())
```

`Node::Instance` retains the definition identity and type arguments rather than
reducing every nominal type to a primitive. `Node::Record` retains required and
read-only fields and its undeclared-item policy. Intersections retain Python's
truthy/falsey refinements; `NewType` nodes retain their immediate nominal base.
Unsupported forms (for example callable values, variadic type-variable tuples and internal
generic Top/Bottom materializations)
remain explicit nodes. A graph containing them is not a complete usable contract.
Nominal identities are scoped to the supplied source unit, not globally stable
catalog IDs; the caller must bind source/stub identity to its catalog revision.

## Branch evidence

`analyze_branches(BranchRequest)` checks a predicate in a typed lexical environment
and observes each requested expression on both successors. Graph roots are ordered
as the true-branch subjects followed by the false-branch subjects. The same checker
owns Boolean composition, comparisons, `isinstance`, unions and attribute narrowing.
There is no predicate-pattern evaluator and no Python execution. Imports, stubs,
source ranges and graph/target budgets use the ordinary analysis boundary. Binding
names are ASCII identifiers; predicates, annotations and subjects must occupy exact
expression ranges in the generated Python context. An unreachable observation is
`Never`; unsupported evidence remains explicit.

Consumers bind these types to their own immutable input provenance. They must not
use evidence for a different predicate, input contract, scope or repeated effectful
call. A consumer may intersect evidence with its original materialized contract to
retain domain constraints, wire representations and field presence.
