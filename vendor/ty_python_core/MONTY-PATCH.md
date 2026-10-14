# Nested collection mutation tracking

Source: crates.io `ty_python_core` 0.0.14, under its original MIT license.
The vendored source and dependency versions are unchanged except for the
semantic-index patch in `src/builder.rs`.

Mutation receivers may traverse container lookups before reaching a method
call or subscript assignment. Retain the owning initializer's statement
dependency through those lookups. Traversal is iterative and only follows
container axes; ordinary reads do not acquire mutation dependencies.

The matching semantic inference patch projects receiver constraints back
through the same axes. Native regression coverage lives in `monty-analysis`;
Plasm's abstract-fixture tests cover admission, execution and field provenance.
