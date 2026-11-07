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
buck2 run //:phoebe -- run examples/hello.phoebe
buck2 build //crates/core:phoebe_core
buck2 test //crates/core:unit_tests //crates/runtime:unit_tests //crates/native:unit_tests
buck2 test //:source_traffic_tests
```

The root `BUCK` and each `crates/*/BUCK` explicitly declare first-party
dependencies. Buck2 can build independent crates concurrently and cache
their outputs. The configuration currently uses Buck2's bundled prelude and
system demo toolchains. No remote cache is configured. Build inputs and
toolchain versions must be pinned before treating cache hits across machines
as reproducibility evidence. Buck2 ignores Cargo's `target/` tree so running
Cargo checks does not churn its file watcher.

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
cargo build --lib
```

The CLI's standalone executable path links a Cargo-produced `libphoebe.a`
runtime archive. A local Buck-built compiler can build that archive from a
repository checkout. Packaging the runtime archive as a Buck2 target is
future work.
