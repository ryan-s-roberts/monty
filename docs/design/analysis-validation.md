# Analysis validation

The analysis API is synchronous and in-process.
The former analysis protocol, pool endpoint, telemetry and transport-specific tests have been removed.
Existing execution-pool tests remain applicable to execution isolation.

Required checks:

- `cargo test -p monty-analysis -p monty-proto --features monty-proto/worker`
- `cargo test -p monty-type-checking`
- `cargo test -p monty-pool --test pool_test` against the rebuilt worker
- `make generate-proto` reproduces the unchanged execution protocol.
- Plasm static admission and structured-analysis tests run without a worker.
- Python profile tests still execute through the pool after in-process checking.

See Plasm's MONTY-EXPRESSION-CUTOVER.md for current measured evidence.
