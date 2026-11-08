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

## Native object cache experiment

On 2026-11-07, a macOS ARM64 debug build with Rust 1.95.0 tested a proposed
disk cache for `NativeBuildSession`. It saved exact per-function Core keys and
Cranelift objects, then loaded them in a separate CLI invocation. The prototype
was discarded: object reuse was slower than one-shot emission on these inputs.

| Program | One-shot build | Warm cached build | Cold cache fill |
| --- | ---: | ---: | ---: |
| Seven-function package graph | 41.5 ms median of 5 | 80.5 ms median of 5 | not timed after tuning |
| 100-function direct call chain | 69.0 ms median of 3 | 102.5 ms median of 3 | not timed |
| 1,000-function direct call chain | 164.2 ms, one run | 247.3 ms, one run | 5,187.2 ms, one run |

The source was parsed and checked on every invocation in both paths. The
cached path linked one object per direct function; the one-shot path emitted
one object for all direct functions. A SHA-256 digest of the compiler binary
added about 35 ms in the optimized-hash prototype on this debug build, but
the many-object link and cache bookkeeping also mattered. The 1,000-function
chain was generated in a temporary file and is not a representative user
program. These are local observations, not cross-platform benchmarks. The
in-process object reuse API remains as an experimental witness. The later
exact-program CLI cache below avoids the many-object link on a warm hit.

## Exact-program CLI object cache

On 2026-11-07, the macOS ARM64 debug compiler built two inputs through
separate CLI processes. Each path parsed and verified source and linked the
executable. The warm cache reused the complete emitted object set when the
span-free checked Core program matched. Seven runs per path followed one
cache-fill build; the table reports medians.

| Program | One-shot build | Warm object-cache build |
| --- | ---: | ---: |
| Six-function module project | 36.0 ms | 34.4 ms |
| Generated 120-function direct call chain | 51.7 ms | 36.0 ms |

The compiler stamp uses local executable file metadata, and cached objects
carry checksums. The chain returned `120` after the cached build. These
measurements include process startup and the host C linker, and do not time
an edited-program rebuild. The six-function difference is small; neither
result establishes a cross-platform speedup or a persistent typed-interface
cache.

## In-process link reuse

On 2026-11-08, the macOS ARM64 debug compiler ran one cold and 20 unchanged
`build` triggers through each `phoebe build-session` process. The module
fixture was `examples/multi_module/`; the channel fixture used
`examples/channel_exchange.phoebe` as the root module of a temporary project.
Both produced executable output. Each warm trigger still loaded the project,
verified Core, and checked native object keys, but reused the linked file.

| Project | First build | Median warm trigger | Warm range |
| --- | ---: | ---: | ---: |
| Six-function modules | 57.5 ms | 0.54 ms | 0.51–0.80 ms |
| Bounded channel exchange | 111.1 ms | 1.09 ms | 1.06–1.35 ms |

The cold channel result includes a C link and checks the runtime archive's
file stamp. Warm triggers use cached output digests when file identity and
change metadata are unchanged. These are no-op rebuild measurements in one
long-lived process; they do not measure edited code, fresh process startup, release builds, or
cross-machine performance.

A separate generated 1,000-function direct chain on the same host had a
169.6 ms one-shot median and a 47.5 ms warm-cache median over three runs per
path. After removing an unnecessary durability sync, three builds into fresh
cache directories took 397.8, 172.7, and 172.0 ms (median 172.7 ms). The
cached executable returned `1000`. This chain stresses code emission and is
not representative of a typical Phoebe program.

## Reachable native emission

On the same 2026-11-07 debug build, a generated chain of 1,000 functions was
built with all functions reachable from `main`, then with `main` returning a
constant so the chain was unreachable. Both builds still parsed and verified
the entire source. The median of five separate CLI invocations was 159.2 ms
when reachable and 52.0 ms when unreachable. This measures two different
programs and does not isolate the effect of the code change against an old
compiler binary. It shows the current standalone path avoids native emission
and linking work for the dead chain on this fixture.

## Automatic loop checkpoint cost

On 2026-11-07, three debug-built compiler revisions on macOS ARM64 emitted
standalone executables for the same `while` loop counting to 100 million.
Twenty timed executions of each binary were interleaved after one warmup;
each run printed and checked the same result. The numbers include process
startup and output. The loop contains no `spawn`.

| Compiler revision | Loop policy | Median run time |
| --- | --- | ---: |
| `0c3ff2f` | No automatic checkpoint | 36.6 ms |
| `e23bc8a` | Checkpoint every 1,024 backedges | 81.7 ms |
| Conditional policy in this revision | Elide checkpoints for a disconnected source component | 36.0 ms |

The all-loop policy more than doubled time on this fixture. The conditional
policy returned to roughly the prior timing, within local run variation.
These are local measurements of one executable, not a general throughput
claim. The source call graph now leaves a loop in a component disconnected
from `spawn` on the direct path. Components joined by a shared helper still
receive checkpoints even when some entry paths never create a task.

## Bounded channel standalone run

On 2026-11-08, an optimized Cargo-built compiler on macOS ARM64 built
`examples/bench_channel_throughput.phoebe`, which sends 10,000 `I64` values
through a one-slot channel from a child task to its parent. The executable
returned and printed `49995000`. The interpreter, native JIT, and standalone
executable agree on that result in `tests/source_traffic.rs`.

```sh
cargo build --release --locked -p phoebe -p phoebe-runtime
python3 examples/bench_standalone.py examples/bench_channel_throughput.phoebe 49995000 --builds 20 --runs 30
```

Three independent invocations of the harness each measured 20 fresh builds
without `--cache-dir`, followed by three warmup runs and 30 timed process
runs. Their within-invocation medians ranged from 40.3 to 40.8 ms for a
build and 2.66 to 2.78 ms for a standalone run. The build measurement starts
a new compiler process, parses and checks the source, generates code, and
links the executable. It excludes building Phoebe itself. The runtime
measurement includes process startup and printing, so it is not a per-message
latency or throughput figure. These local timings are a reproducible baseline
for this one workload, not a claim of OCaml-like compilation or general native
runtime speed.
