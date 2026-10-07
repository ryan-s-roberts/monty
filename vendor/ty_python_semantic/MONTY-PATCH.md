# Structured type export patch

Source: ty_python_semantic 0.0.14, astral-sh/ruff commit
62914c4b9b79a9e5004374a9c482ad2ed69290e1 (published crate source).
License files are retained. No inference rules are changed.

The added types::export module exposes bounded, owned structured inference
results to monty-analysis without parsing diagnostic display text.
