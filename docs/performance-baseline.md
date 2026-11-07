# Initial compiler timing baseline

Status: local measurements on 2026-11-06, not a speed claim for Phoebe 0.0.1.
The commands below used an optimized Rust build on macOS ARM64. Each line is
the median of 30 rounds. The source file was read before timing; each round
reparsed, lowered and verified, compiled a fresh Cranelift JIT module, then
invoked its `main` 1,000 times through `JitExecutable::run`.
`compile_jit` currently verifies the Core IR again, so the JIT column includes
that repeated verification and module setup as well as code generation.

```sh
cargo run --release --example bench -- examples/hello.phoebe 30
target/release/examples/bench examples/nested_result.phoebe 30
```

| Program | Parse | Lower + verify | JIT compile | Wrapper + native invocation |
| --- | ---: | ---: | ---: | ---: |
| `hello.phoebe` | 0.001 ms | 0.002 ms | 0.031 ms | 8.8 ns/call |
| `nested_result.phoebe` | 0.012 ms | 0.027 ms | 0.164 ms | 15.6 ns/call |

The invocation column includes the Rust wrapper and a tiny native function;
it is not a throughput measurement for a useful workload. File I/O, project
loading, object emission, linking, cold process startup, allocator pressure,
and incremental rebuilds are excluded. The two examples do not establish
scaling, OCaml-like compile times, or native runtime speed for concurrent
programs. Re-run the command on representative programs and target machines
as those features land. No benchmark result is used as a correctness check.

## Warm project session

On the same macOS ARM64 host, an optimized build ran 100 warm rounds of
`examples/multi_module/` with six functions:

```sh
cargo run --release --locked --example bench_project -- examples/multi_module 100
```

The median fresh project load plus Core lowering was 0.056 ms. The median
in-process session compile was 0.054 ms, reusing six checked Core bodies.
Each round compared the printable Core IR from both paths. These timings
include source reads, manifest parsing, AST qualification, Core checking,
and cache bookkeeping. They exclude native code generation and linking.
The difference is too small to establish a useful speedup on this fixture;
larger projects and edited-module rebuilds need measurement.
