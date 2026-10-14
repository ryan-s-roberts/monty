# Structured type export patch

Source: ty_python_semantic 0.0.14, astral-sh/ruff commit
62914c4b9b79a9e5004374a9c482ad2ed69290e1 (published crate source).
License files are retained.

The added types::export module exposes bounded, owned structured inference
results to monty-analysis without parsing diagnostic display text.

Nested collection mutations retain initializer dependencies through container
lookups, using the paired `ty_python_core` source patch. Receiver evidence is
projected back through each container axis and supplies type context to nested
literal initializers. Resolved scalar collections still reject incompatible
updates; unresolved or recursive values never become `Any`.
