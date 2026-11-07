# Build graph

Phoebe has seven small library crates and a root facade/CLI. Dependencies
point in one direction:

```text
syntax ───────┬──── project
              ├──── core ───── frame ───── native ──── phoebe CLI
capability ───┘       │                    │
runtime ──────────────┴────────────────────┘
```

- `phoebe-syntax`: spans, diagnostics, lexer, parser, and source AST.
- `phoebe-project`: manifest and module loading.
- `phoebe-capability`: independent capability-flow facts and verifier.
- `phoebe-runtime`: scheduler, channels, reply rights, and native ABI.
- `phoebe-core`: checked IR, lowering, verification, and interpreter.
- `phoebe-frame`: suspension liveness and physical frame layout.
- `phoebe-native`: Cranelift JIT, object emission, direct and retained codegen.
- Root `phoebe`: compatibility facade, CLI, and Cargo static library.

Within Core, IR definitions stay in `lib.rs`; lowering, verification, and the
interpreter are separate modules. Within Runtime, the native ABI is separate
from the scheduler. Within Native, direct codegen and retained polling are
separate from the JIT/object entry points. These are source boundaries; the
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
buck2 build //crates/core:phoebe_core
buck2 test //crates/core:unit_tests //crates/runtime:unit_tests //crates/native:unit_tests
buck2 test //:source_traffic_tests
buck2 test //:source_spawn_tests
```

The root `BUCK` and each `crates/*/BUCK` explicitly declare first-party
dependencies. Buck2 can build independent crates concurrently and cache
their outputs. The configuration currently uses Buck2's bundled prelude and
system demo toolchains. No remote cache is configured. Build inputs and
toolchain versions must be pinned before treating cache hits across machines
as reproducibility evidence. Buck2 ignores Cargo's `target/` tree so running
Cargo checks does not churn its file watcher.
`rust-toolchain.toml` selects Rust 1.95.0. The Linux CI workflow downloads
Buck2 2026-09-15 and Maude 3.5.1 from fixed release assets and verifies their
SHA-256 digests. It runs workspace Cargo checks, all Buck2 tests, and the
bounded formal suite. These pins constrain CI tools, but the system C linker
and Buck2 demo toolchains are not yet hermetic across hosts.

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

Cargo remains the source of truth for dependency resolution and test running:

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
