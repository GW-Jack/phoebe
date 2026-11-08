# Phoebe

Phoebe is an early language design for native programs with task-local memory,
lightweight tasks, and value-passing channels. The repository has draft
specifications, bounded formal models, and a first executable compiler slice.
It does not yet have all features required for 0.0.1.

The cooperative runtime kernel exercises scope-owned task frames,
bounded `I64` and request channels, and one-shot `ReplyTo<I64>` rights;
`docs/runtime-kernel.md` states its boundary.
Checked Core and source programs can create and close scalar channels in the
interpreter, Cranelift JIT, and standalone executables. All three paths run
the source `spawn`, `send`, and `receive` exchange with retained frames and
bounded channels. Retained native code still supports a constrained Core
subset; `docs/native-frames.md` describes its limits.
Standalone channel builds link the `libphoebe_runtime.a` archive. Buck2
builds and attaches it to the CLI; Cargo can produce it from the runtime
crate for local development. A packaged compiler must include the archive.
Source programs can create `reply<I64>()?`, transfer `ReplyTo<I64>` to a
spawned task, complete it once, and receive or cancel the reply. The
`examples/reply_exchange.phoebe` fixture runs in the interpreter, JIT, and
standalone executable.
`examples/request_exchange.phoebe` transfers a reply right through an owned
request channel across the same three backends.
`examples/request_backpressure.phoebe` sends two owned requests through a
one-slot queue, parking and resuming both sides.
Source `scope { ... }` joins tasks started inside the block, including their
descendants. Early `?` exits join before returning. A scope that cannot spawn
is erased from Core; `examples/scoped_spawn.phoebe` shows the source form.
`Unit` is a zero-component result for functions and scoped tasks that return
no useful value; `examples/unit_tasks.phoebe` exercises a suspended `Unit`
call and task across all three execution paths.

## Current milestone

`docs/version-0.0.1.md` states the release gates and the exact subset that
currently runs. The Rust workspace parses functions over I64, Bool, Unit, nominal sums
and products with primitive, earlier nominal, or communication capability fields.
`examples/capability_product.phoebe` closes a nested product with channel and
request rights after an owned call.
It supports arithmetic, conditions, exhaustive `when`, mutable locals, loops,
concrete `Option<T>` values, call-site scalar borrows, and `?` on concrete
`Result<T, E>` applications with finite payloads. Nominal values have affine move
checking across branches and loops, and `copy(x)` records an explicit
fixed-size copy cost. The compiler then
lowers them to checked Core IR. Programs run
through an interpreter or Cranelift JIT. The compiler can also
build a standalone executable for a zero-argument `main` using Cranelift
object output and the host C linker.
Local projects can use a `phoebe.toml` manifest and explicit module imports;
`examples/multi_module/` runs in both engines and builds a standalone binary.
`docs/modules.md` describes the format and current limits.
`docs/core-ir.md`, `docs/affine-ownership.md`, and `docs/call-borrows.md`
describe the current IR and its limits.
`phoebe frames <file|project>` prints planned suspension spill sets;
`docs/native-frames.md` explains their current evidence and the remaining
native poll ABI work.
`phoebe shapes <file|project>` prints checked parameter and result transport
shapes. `docs/generic-core.md` explains the additional proof and ABI work
needed before generic functions can share native bodies by shape.
The isolated `phoebe-generic-core` crate now verifies a first opaque-owner
IR slice. `phoebe generic-ir <file>` lowers identity, forwarding, and
generic-to-generic calls into printable parametric Core. The module verifier
checks call substitutions and owner transfer. Concrete call sites and native
emission are still unimplemented.

Build and run the compiler with Buck2:

```sh
buck2 build //:phoebe_dist
buck2 run //:phoebe -- run examples/hello.phoebe
buck2 run //:phoebe -- run examples/boolean_short_circuit.phoebe
buck2 run //:phoebe -- run examples/option.phoebe
buck2 run //:phoebe -- run examples/scoped_spawn.phoebe
buck2 run //:phoebe -- run examples/with_pair.phoebe
buck2 run //:phoebe -- run examples/channel_exchange.phoebe
buck2 run //:phoebe -- run examples/reply_exchange.phoebe
buck2 run //:phoebe -- run examples/request_exchange.phoebe
buck2 run //:phoebe -- run examples/request_backpressure.phoebe
buck2 run //:phoebe -- run examples/capability_product.phoebe
buck2 run //:phoebe -- run examples/with_product.phoebe
buck2 run //:phoebe -- run examples/multi_module
buck2 run //:phoebe -- shapes examples/product.phoebe
buck2 run //:phoebe -- generic-ir examples/generic_identity.phoebe
buck2 run //:phoebe -- build examples/channel_exchange.phoebe -o /tmp/phoebe-exchange
/tmp/phoebe-exchange
buck2 test //...
```

`docs/spec.md` states the proposed language contract and marks open decisions.
`docs/non-pessimization.md` defines cost events and example IR acceptance
criteria for the future compiler.
`docs/compiler-architecture.md` records the research-informed proof and
hackability plan.
`docs/build-system.md` describes the crate graph, Buck2 targets, dependency
pinning, and development commands.

Cargo remains available for tests and standalone runtime archives:

```sh
cargo test --workspace
cargo build -p phoebe-runtime --lib
```

`docs/decisions/0001-compiler-principles.md` records the accepted compiler
principles. `docs/decisions/0002-concurrency-core.md` adopts scoped tasks and
channels with move-only communication capabilities in messages, and actors as
an abstraction over that core. `docs/pony-actors.md` explores the actor details.
`docs/decisions/0003-language-baseline.md` records the agreed type, borrow,
error, module, and ABI-shape generic policies; `docs/language-features.md`
develops them and lists the next questions.
`docs/decisions/0004-structural-capability-cleanup.md` specifies cleanup of
communication capabilities inside rejected or discarded messages.
`docs/concurrency-pressure-tests.md` compares scoped tasks and actors on
stateful and asynchronous workloads and records the resulting design gates.
`formal/core.maude` explores one task with nested regions and a reference.
`formal/two_task.maude` explores parent and child tasks, explicit handle fork,
transfer at spawn, channel delivery, close, and child failure.
`formal/reply_capability.maude` explores a one-shot reply right moving through
a request, cancellation, discard, and failure. `formal/retained_borrow.maude`
explores a nested call that forwards a borrow across a channel wait. All four
models have finite state spaces. See `formal/README.md` for the exact
models and their limits. These checks do not prove the full language, compiler,
or native runtime safe.

Run the checks with:

```sh
python3 formal/check_models.py
```

The script checks the expected safety-search and witness results from Maude.
`docs/performance-baseline.md` records initial release-mode compiler phase
measurements and their limits.
