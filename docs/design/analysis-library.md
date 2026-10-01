# Standalone analysis library

Static analysis is synchronous compiler work.
Plasm links `monty-analysis` directly; there is no analysis pool endpoint or protocol extension.
The library depends on the existing checker and shared owned types, not the interpreter.

```text
Plasm compiler -> monty-analysis -> monty-type-checking -> ty export
Plasm execution -> monty-pool -> worker -> Monty interpreter
```

Each analysis call creates a fresh in-memory checker and never evaluates Python.
Exact UTF-8 target spans select expression roots; empty targets check definitions only.
Results preserve nominal identity, unions, literals, recursive graphs and record presence.
Unknown and unsupported results are explicit, never permission to execute.
The caller seals captures, domain contracts and effect authority independently.

The pinned ty_python_semantic 0.0.14 export patch exposes owned types without changing inference.
Salsa and its macro-rules dependency are pinned exactly to the tested 0.28.2
because Ruff uses their unstable API; a consuming workspace must not resolve a
newer macro implementation against the older runtime.
All consuming Cargo roots select the same patch; no registry source is edited.
Public release requires a resolvable version of that export API.

The server schedules checking on a bounded CPU executor.
Cancellation can discard a result but does not forcibly stop an in-process thread.
Source, AST and graph limits remain enforced; there is no subprocess deadline or allocator claim.
The execution pool retains its existing isolation and protocol unchanged.
