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
