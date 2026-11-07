# Version 0.0.1 implementation contract

Status: work in progress. This is the implementation target for the first
usable Phoebe release, not a claim that the current repository meets it.

## Release gates

1. Parse and type-check a documented source subset with functions, nominal
   data, exhaustive branching, local bindings, explicit borrows and moves,
   `Result`/`?`, and bounded task/channel operations. Unsupported syntax must
   receive a source-located diagnostic, not an internal panic.
2. Lower that subset to typed, printable Core IR with block parameters,
   explicit places, regions, capability transitions, cleanup edges, and cost
   events. Provide an independently useful IR verifier and interpreter.
3. Generate native code only through Cranelift. Interpreter and native runs
   must agree on representative successful and failing programs, including
   ownership and channel behavior. A release build must be runnable without
   the compiler process.
4. Make task-local storage, borrows across suspension, bounded channels,
   `ReplyTo`, cancellation, failure cleanup, and scope joins safe by
   construction. The runtime must not need a general garbage collector.
5. Support a minimal manifest, pinned dependency graph, modules, and
   incremental interface-based rebuilds. The compiler must remain easy to
   inspect with textual IR, source spans, and isolated pass execution.
6. Run the formal checks, compiler tests, differential tests, and cost
   fixtures. State precisely which invariants are proved, model-checked,
   tested, or still assumptions. Measure compile and runtime behavior rather
   than claiming OCaml-like times or native speed from architecture alone.

These are release gates for a useful first version of the design. Not every
future surface feature or optimization must land in 0.0.1, but memory safety,
scoped concurrency, and the compiler's cost discipline cannot be represented
only by prose at release.

## Current executable slice

The Rust crate currently parses functions over I64, Bool, nominal sums with
at most one payload per variant, and nominal products with finite fields.
User-declared payloads and fields may contain earlier nominal types; recursive
data and concrete `Result` fields in user declarations remain unsupported.
It supports up to four owned parameters (I64 borrows only), `val` and
`var` bindings, assignment, `while`, integer and boolean literals, unary
negation, `+`, `-`, `*`, `==`, `<`, `if` expressions with block arms, exhaustive
`when` expressions, acyclic direct calls, call-site `&I64` and `&mut I64`
arguments, `?` on concrete built-in `Result<T, E>` applications with
primitive or existing nominal payloads, and a matching tail expression.
Nominal values have affine move checking:
owning uses and nominal field projections consume them; primitive product
field reads preserve them. CFG joins reject later reuse when any incoming path
has moved the owner. `copy(x)` makes
an explicit independent nominal copy and records a `copy.value` cost site.
Recursion is rejected until task stacks have safe growth limits. Arithmetic
wraps at 64 bits in both interpreter and native execution. `phoebe check`,
`phoebe ir`, `phoebe cost`, `phoebe run`, and
`phoebe build <file> -o <output>` work on that subset.
`phoebe clif <file>` prints the pre-legalization Cranelift IR. The scalar
`identity` cost fixture is an executable acceptance test; the other cost
fixtures remain design sketches. `docs/performance-baseline.md` records a
local phase-timing snapshot without a general speed claim.
Native execution uses Cranelift JIT or Cranelift object emission linked with
the host `cc`; `--interp` selects the reference interpreter. The built
executable runs without the compiler process and prints the I64 returned by
zero-argument `main`. External JIT and interpreter entries also require I64
signatures; calls within Phoebe can pass and return Bool or nominal data.
Finite products also cross calls and returns by flattened scalar
components, without native heap allocation. `docs/abi-shapes.md` records the
current internal calling convention and its missing cost threshold. Core IR
has typed values, conditional branches, variant matches, block parameters,
and task-local places for `var`. Nested `Result` applications use a finite
flattened ABI shape; see `examples/nested_result.phoebe`. Its verifier checks
definition uniqueness, dominance, place initialization, affine moves,
block-argument types, call targets and modes, and return
types. Source spans survive parsing and lowering. `docs/core-ir.md` and
`docs/call-borrows.md` explain the current operations and their evidence
boundary. Borrows are confined to synchronous calls; general reference
lifetimes and regions are not implemented. `docs/result-try.md` states the
current `?` rule and its missing user-declared `Result` fields, generic
functions, and cleanup work.

This slice establishes the parser → checked IR → interpreter/Cranelift path.
It does not yet implement general ownership, concurrency, or all release
gates above. A local `phoebe.toml` manifest and explicit imports now compile
multiple modules together; `examples/multi_module/` runs in both engines and
builds a standalone executable. `docs/modules.md` describes that path and
the absent external dependency, interface cache, and incremental rebuild
support. The `examples/cost/` files other than scalar `identity` remain
design fixtures, not executable acceptance tests.

`src/runtime.rs` implements a cooperative scheduler
kernel for one scope, bounded `I64` and request channels, and a one-shot
`ReplyTo<I64>` right. Its tests cover parking, backpressure, FIFO delivery,
handle transfer, reply transfer through a request queue, rejected sends,
queued request discard, cancellation, close, failure cleanup, and deadlock
cleanup. Recoverable runtime calls return the original frame or request on
pre-commit failure. Checked Core uses this kernel for channel operations and
task creation; source lowering currently uses it for channel creation and close.
Typed resource creation errors now distinguish invalid capacity, table
limits, and allocation failure without publishing partial endpoint pairs.
`docs/runtime-kernel.md` describes the kernel and the compiler,
general typed-message, and native-runtime integration still required.
`src/capability_flow.rs` separately checks exact slot ownership in abstract
control-flow graphs and derives failure/suspension live-slot maps. Core IR
now produces those graphs for hand-built `channel.new.i64`, endpoint closes,
and capability jumps. The interpreter, Cranelift JIT, and standalone object
code execute creation and close through the bounded scheduler and follow a
typed resource-failure edge. Source now supports
`val (sender, receiver) = channel<I64>(capacity = n)?` inside a function
returning `Result<_, ResourceError>`, with explicit endpoint `close()`.
`phoebe check` runs the exact capability verifier and rejects leaked or
twice-closed endpoints. Source `?` now closes exactly the live bare endpoints
on its early error return; ordinary returns still require explicit
disposition. Source accepts owned `Sender<I64>` and `Receiver<I64>` function
parameters and `spawn worker(sender)?;` as a fallible statement. On success
the child owns its argument; on task-limit failure the parent retains it and
the `?` cleanup closes it. This path currently runs in the interpreter only.
General source capability safety, send/receive, lexical scope
cleanup, and cancellation cleanup remain open. `docs/capability-ir.md` specifies
the integration boundary.
The versioned runtime ABI now has send and receive calls with explicit
`Pending` statuses and no endpoint transfer. Scheduler tests show wake and
retry across both operations. Cranelift still needs generated task frames
before source send/receive can use those calls in both execution engines.
Core now checks scalar send/receive terminators and derives live-right maps
for retry suspension. Its interpreter retains frames and places across a
pending operation, and executes buffered scalar traffic. Core `spawn` now
transfers endpoint and closed nominal arguments only on success. A two-task
Core program parks a parent receiver, wakes it from a child send, and handles
a task-limit failure without losing the parent's rights. Native retained
frames and source channel traffic remain open. A Rust host test also spawns a
checked Core receiver that parks, wakes on a send, and returns its value.
Internal Core calls can transfer an endpoint to a callee that closes it;
interpreter and JIT executions agree on that synchronous path.

## Commands

```sh
cargo run -- run examples/hello.phoebe
cargo run -- run examples/branch.phoebe
cargo run -- run examples/loop.phoebe
cargo run -- run examples/borrow.phoebe
cargo run -- run examples/variant.phoebe
cargo run -- run examples/product.phoebe
cargo run -- run examples/payload.phoebe
cargo run -- run examples/try.phoebe
cargo run -- run examples/nested_result.phoebe
cargo run -- run examples/nested_variant.phoebe
cargo run -- run examples/copy.phoebe
cargo run -- run examples/channel_create.phoebe
cargo run -- run examples/channel_cleanup.phoebe
cargo run -- run examples/spawn_close.phoebe --interp
cargo run -- cost examples/copy.phoebe
cargo run -- run examples/hello.phoebe --interp
cargo run -- ir examples/hello.phoebe
cargo run -- build examples/borrow.phoebe -o /tmp/phoebe-borrow
/tmp/phoebe-borrow
cargo test
python3 formal/check_models.py
```
