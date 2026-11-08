# Build graph

Phoebe has nine library crates and a root facade/CLI. Dependencies point in
one direction:

`package` and `syntax` feed `project`. `syntax`, `capability`, and `runtime`
feed `core`; `core` feeds `abi` and `frame`; `abi`, `core`, `frame`, `runtime`,
and `syntax` feed `native`. The root facade and CLI sit atop this graph.

- `phoebe-syntax`: spans, diagnostics, lexer, parser, and source AST.
- `phoebe-package`: content-pinned local dependency graph and lockfile.
- `phoebe-project`: manifest and module loading.
- `phoebe-capability`: independent capability-flow facts and verifier.
- `phoebe-runtime`: scheduler, channels, reply rights, and native ABI.
- `phoebe-core`: checked IR, lowering, verification, and interpreter.
- `phoebe-abi`: backend independent flattened transport shapes.
- `phoebe-frame`: suspension liveness and physical frame layout.
- `phoebe-native`: Cranelift JIT, object emission, direct and retained codegen.
- Root `phoebe`: compatibility facade and CLI.

Within Syntax, `syntax/ast.rs` defines the public tree and `syntax/lexer.rs`
owns tokens. The parser keeps its shared cursor in `syntax/parser.rs`, with
type, declaration, statement, and expression parsing in sibling modules under
`syntax/parser/`. The `syntax.rs` facade preserves the public tree and `parse`
API. Within Core, IR definitions stay in `lib.rs`; statement lowering lives in
`lowering/body.rs`, expression lowering in `lowering/expr.rs`, nominal
registration in `lowering/data.rs`, incremental compatibility in
`lowering/reuse.rs`, and function assembly in `lowering.rs`. Core verification
keeps the ownership pass in `verification.rs`, with separate operation typing,
structural, and capability-flow modules; the interpreter is separate. Within
Runtime, the native ABI is separate from the scheduler. Within Native, direct
codegen, retained polling, and
in-process object reuse live in separate modules. Native reachability selection lives
in `reachability.rs`. These are source boundaries; the
crate boundaries determine independent build and cache units.

## Buck2

Install a compatible Buck2 and Rust toolchain, then run from the repository
root:

```sh
buck2 build //:phoebe
buck2 build //:phoebe_dist
buck2 run //:phoebe -- run examples/hello.phoebe
buck2 build //crates/runtime:runtime_archive
buck2 run //:phoebe -- build examples/channel_exchange.phoebe -o /tmp/phoebe-exchange
buck2 run //:phoebe -- build examples/multi_module -o /tmp/phoebe-app --cache-dir /tmp/phoebe-objects
buck2 run //:phoebe -- build-session examples/multi_module -o /tmp/phoebe-app
buck2 run //:phoebe -- build examples/package_graph/app -o /tmp/phoebe-app
buck2 build //crates/core:phoebe_core
buck2 test //crates/core:unit_tests //crates/runtime:unit_tests //crates/native:unit_tests
buck2 test //:compiler_tests
buck2 test //:source_traffic_tests
buck2 test //:source_replies_tests
buck2 test //:source_requests_tests
buck2 test //:core_requests_tests
buck2 test //:source_spawn_tests
buck2 test //:source_with_tests
buck2 test //:source_yield_tests
buck2 test //:source_unit_tests
buck2 test //:source_fork_tests
buck2 test //:project_tests
buck2 test //:build_session_cli_tests
buck2 test //:cost_acceptance_tests
buck2 test //...
```

The root `BUCK` declares the CLI and facade. `build_defs/integration_tests.bzl`
declares each Cargo integration suite as its own Buck2 target in the root
package. Keeping these targets in the root package preserves relative
`include_str!` paths from `tests/` to `examples/`; each suite still lists only
the example files it reads. Each `crates/*/BUCK` declares its crate tests.
Buck2 can build independent
crates concurrently and cache
their outputs. The configuration currently uses Buck2's bundled prelude and
system demo toolchains. No remote cache is configured. Build inputs and
toolchain versions must be pinned before treating cache hits across machines
as reproducibility evidence. Buck2's project ignore rules exclude Cargo's
`target/` tree and Git metadata from build inputs. Its event log can still
report raw filesystem notifications for those paths.
`rust-toolchain.toml` selects Rust 1.95.0. The Linux CI workflow downloads
Buck2 2026-09-15 and Maude 3.5.1 from fixed release assets and verifies their
SHA-256 digests. It runs workspace Cargo checks, all Buck2 tests, and the
bounded formal suite. These pins constrain CI tools, but the system C linker
and Buck2 demo toolchains are not yet hermetic across hosts.

`build-session` keeps parsed modules, checked Core functions, and direct native
objects in one compiler process. It prints `ready`, then accepts one `build`
line for each requested rebuild and `quit` to exit. Every build response
reports parsed modules, changed interfaces, reused Core functions, reused
native objects, and elapsed build time. Source edits between `build` lines
are read from disk. A
failed source edit prints an error and leaves the last linked executable in
place. This is a trigger-driven local session; it does not watch files or
persist typed interfaces across compiler processes. Retained poll objects can
also be reused when their reachable code and data layouts are unchanged. The
separate `--cache-dir` mode of one-shot
`build` persists exact-program native objects across processes.

`third-party/rust/Cargo.toml` and its lockfile pin third-party Rust versions.
Reindeer generates `third-party/rust/BUCK` and sha256-checked archive rules.
To refresh after changing those dependencies:

```sh
cd third-party/rust && cargo update && cd ../..
reindeer -c reindeer.toml buckify
```

The `fixups/` directory records build-script settings needed by crates whose
Cargo builds generate code or environment values. Commit the manifest,
lockfile, generated BUCK file, and fixups together.

## Cargo and checks

Cargo remains the source of truth for dependency resolution and provides a
second route for workspace tests:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p phoebe-runtime --lib
```

The CLI's standalone executable path links `libphoebe_runtime.a`. Buck2's
`//crates/runtime:runtime_archive` target builds it from the dependency-free
runtime crate and attaches it as a relocatable resource to `//:phoebe`.
The compiler resolves Buck2's resource map at runtime, so a Buck2 build does
not invoke Cargo to build standalone programs. Cargo builds the same archive
for its development CLI and tests.
`//:phoebe_dist` copies the compiler and archive into one directory for
relocation or packaging. A relocated copy built a standalone channel exchange
in a local smoke test; the archive remains a host-specific artifact built by
the current system Rust toolchain.
