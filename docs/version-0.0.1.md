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

The Rust workspace currently parses functions over I64, Bool, Unit, nominal sums with
at most one payload per variant, and nominal products with finite fields.
User-declared payloads and fields may contain earlier nominal types and finite
concrete `Option` and `Result` applications; recursive data remains unsupported.
`Unit` has no native transport components; a `Unit` function block may omit its
final expression. Spawned tasks may return `Unit`, and a suspending call may
receive a zero-word `Unit` result. `examples/unit_tasks.phoebe` and
`examples/unit_data.phoebe` run in the interpreter, JIT, and standalone paths.
Zero-width product fields can be read from mutable task-local places before
or after suspension; `examples/unit_place.phoebe` exercises the latter.
It supports up to four owned parameters (I64 borrows only), `val` and
`var` bindings, assignment, `while`, integer and boolean literals, unary
numeric negation, Boolean `!`, `+`, `-`, `*`, `==`, `!=`, `<`, `<=`, `>`, `>=`,
short-circuit `&&` and `||`, `if` expressions with block arms, exhaustive `when` expressions,
acyclic direct calls (a recursive cycle is diagnosed at a participating call
site), call-site `&I64` and `&mut I64`
arguments, `?` on concrete built-in `Result<T, E>` applications with
primitive, existing nominal, or finite `Option`/`Result` payloads and the same
error type as the enclosing function's `Result<U, E>`, and a matching tail expression.
Boolean operators require `Bool` on both sides even when the right side is
skipped at runtime. Their precedence is `||` below `&&`, below comparisons;
the right side is a separate Core CFG branch and may contain `?`.
Comparison operands evaluate left to right; each comparison is one typed Core
operation and one Cranelift integer comparison.
Nominal values have affine move checking:
owning uses and nominal field projections consume them; primitive product
field reads preserve them. CFG joins reject later reuse when any incoming path
has moved the owner. `copy(x)` makes
an explicit independent nominal copy and records a `copy.value` cost site.
Source `spawn` records a `task.create` cost site with its source span; this
ledger identifies static sites rather than counting successful creations.
Recursion is rejected until task stacks have safe growth limits. Arithmetic
wraps at 64 bits in both interpreter and native execution. `phoebe check`,
`phoebe ir`, `phoebe cost`, `phoebe run`, and
`phoebe build <file> -o <output>` work on that subset.
`Option<T>` is a concrete built-in sum with `None` and `Some(T)`, including
finite nesting with `Result` and earlier nominal types. `when` must cover both
variants. `?` accepts only `Result`, with no implicit `Option` conversion.
The parser currently limits nested source constructs and chained expressions
to 48 levels, and combined expression/body tree depth to 64, reporting a
source location at the limit. These bound parser and lowering stack use; they
are implementation limits, not the intended language semantics.
The diagnostic regression suite mutates three valid programs 1,000 times
each and checks that parse/lower errors stay in source bounds. It also covers
deeply nested expressions, types, and branches. This is bounded test evidence,
not a proof for every input.
`phoebe clif <file>` prints the pre-legalization Cranelift IR. The scalar
`identity`, nominal `product_identity`, `unit_identity`, and `empty_scope`
cost fixtures are executable acceptance tests; the collection cost fixtures
remain design sketches. `docs/performance-baseline.md` records a
local phase-timing snapshot without a general speed claim.
Native execution uses Cranelift JIT or Cranelift object emission linked with
the host `cc`; `--interp` selects the reference interpreter. The built
executable runs without the compiler process and prints the I64 returned by
zero-argument `main`. External JIT and interpreter entries also require I64
signatures; calls within Phoebe can pass and return Bool or nominal data.
Standalone runtime failures now print a bounded diagnostic rather than a
generic failure line; a source deadlock has the same reason in interpreter,
JIT, and standalone runs. The C entry keeps its original v1 symbol and adds
diagnostic variants with a caller-owned 512-byte error buffer.
Finite products also cross calls and returns by flattened scalar
components, without native heap allocation. Direct returns wider than the
target's register return capacity use a caller-owned stack buffer, so x86-64
and AArch64 both support finite nested results. `docs/abi-shapes.md` records the current internal
calling convention and its unmeasured argument cost threshold. Core IR
has typed values, conditional branches, variant matches, block parameters,
and task-local places for `var`. `val (a, b, ...) = product.unpack();`
destructures a whole finite product, including nested fields, with one name
per field; the existing request pair binding uses the same syntax.
`examples/product_unpack.phoebe` agrees in interpreter, JIT, and standalone
execution, including a retained task path. Nested `Result` applications use a finite
flattened ABI shape; see `examples/nested_result.phoebe`. Its verifier checks
definition uniqueness, dominance, place initialization, affine moves,
block-argument types, call targets and modes, and return
types. Source spans survive parsing and lowering. `docs/core-ir.md` and
`docs/call-borrows.md` explain the current operations and their evidence
boundary. Borrows are confined to one call chain and can span a channel
suspension. Each borrowed call now has a printed call-scoped loan region with
explicit place or forwarded-reference roots and a return/unwind end. Core
checks the region against the call; frame analysis checks that place roots
survive a suspending call. General reference lifetimes and inferred regions
remain unimplemented.
The CLI now carries an immutable `CheckedProgram` from final Core verification
through interpreter, JIT, object emission, and the optional object cache.
Native child emission accepts a borrowed verification token rather than
rechecking the whole program for each poll entry. Public raw-`Program` APIs
still verify before use. No compile-time speedup has been measured for this
change.
`docs/result-try.md` states the current `?` rule and its remaining generic
function and cleanup work. `examples/result_in_data.phoebe` exercises nested
`Result` fields and variant payloads through all three execution paths.

This slice establishes the parser → checked IR → interpreter/Cranelift path.
It does not yet implement general ownership, concurrency, or all release
gates above. A local `phoebe.toml` manifest and explicit imports now compile
multiple modules together; `examples/multi_module/` runs in both engines and
builds a standalone executable. `docs/modules.md` describes that path and
the in-process parsed-module session with exact source-level interfaces and
checked-Core function-body reuse. The `build-session` CLI keeps compiler and
native sessions alive across explicit rebuild triggers and reports module,
Core body, and object reuse. It does not persist typed interfaces between
processes. The package resolver locks and verifies
transitive local path dependencies by exact source content. Direct dependency
root modules and their transitive calls compile and run in the interpreter,
Cranelift JIT, and standalone executable; see `examples/package_graph/app/`.
Project declarations are private by default; `pub` exports functions and data
types, and the interface tracks private nominal layouts exposed by public
signatures. Interfaces now have a versioned canonical byte encoding and
SHA-256 fingerprint for a future persistent cache; in-process invalidation
still compares exact structures. Public imported nominal variants can be constructed and matched
with qualified names. An in-process `NativeBuildSession` now emits separate
objects for direct functions and reuses them after checked Core edits when
their body and ABI inputs are unchanged, including across unrelated function
insertions or removals and unrelated nominal data insertions. A changed or
removed existing data layout conservatively invalidates the in-process
direct-object cache; standalone one-shot builds retain
single-object emission. In-process retained poll objects now reuse exact
reachable call and spawn
closures after unrelated body edits or function insertions. Their keys ignore
spans and normalize call targets to names; poll-local symbols use those names.
A reachable helper, task, or nominal layout change rebuilds the object.
Unrelated nominal data changes can preserve it. An in-process session skips
the linker when its emitted objects and runtime archive file stamp match the last
successful link and the output file is intact. The optional CLI `--cache-dir`
persists a complete
emitted object set for unchanged codegen inputs, ignoring source spans and
unreachable pure leaf bodies, but still parses, verifies Core,
lowers, and links each invocation. A changed program recompiles that set;
the in-process build session retains finer direct-function reuse. Registry
dependencies and cached typed interfaces remain open. Core reuse now rebases source spans on `spawn` and
`scope.join` as well as instructions and `yield`; direct-object keys ignore
every such span. In-process Core reuse also remaps call and spawn IDs when a
private function is inserted or removed without changing existing signatures
or referenced nominal layouts. It also remaps nominal IDs across unrelated
data insertions, after checking nested layout compatibility. The collection
fixtures in `examples/cost/` remain design sketches.
Standalone emission starts at `main` and omits unreachable direct functions
and their spawned poll children. A dead runtime-using function cannot change
the entry wrapper ABI.

`crates/runtime` implements a cooperative scheduler
kernel for one scope, bounded `I64` and request channels, and a one-shot
`ReplyTo<I64>` right. Its tests cover parking, backpressure, FIFO delivery,
handle transfer, reply transfer through a request queue, rejected sends,
queued request discard, cancellation, close, failure cleanup, and deadlock
cleanup. Recoverable runtime calls return the original frame or request on
pre-commit failure. Checked Core and source lowering use this kernel for
channel operations and task creation in the interpreter.
Each spawned task now records its parent. The Rust runtime lets any task join
its descendant tree without waiting for unrelated siblings; the root joins
all tasks. The Rust runtime also has generation-tagged lexical scope tokens:
an owner can enter a nested scope, spawn into it, and join its transitive
tasks without waiting for preexisting siblings. A task cannot finish
successfully with an open nested scope. The runtime archive has native scope
entry and join calls; a synthetic poll task parks at join and resumes after
its child completes. Hand-built Core now has `scope.enter` and a suspending
`scope.join` with a verifier-enforced region stack. Interpreter, JIT, and
standalone tests cover a pending join and spawn-limit failure. Source
`scope { ... }` is now a statement. Implicit `?` returns close owned rights,
join open scopes from inner to outer, and then return the error. A directed
spawn-effect summary erases scopes that cannot spawn, including calls to pure
helpers. `docs/scope-ir.md` records the invariant and remaining evidence.
Nested scope slots have a caller-settable budget, with 64 slots by default;
the root does not count and closed slots are reused. Source interpreter and
JIT tests exercise a zero-slot fault, while runtime tests check that failed
entry leaves the current scope unchanged.
Fault and deadlock cleanup now retire remaining open scope records after all
tasks and their capabilities have been closed; the runtime verifier rejects
an active nested scope with a terminal owner.
The source differential suite also retains a mutable caller place through a
borrowed callee's pending scope join, then checks the task-limit error path.
The finite Maude lexical-scope model explores a fixed root, earlier sibling,
inner child, and grandchild with independently checked live counts. It does
not establish a refinement from the compiler or runtime to that model.
The Rust spawn path now atomically transfers channel handles and either reply
capability to a child. Native reply and typed spawn shims are exercised by
synthetic poll frames, including a receiver that parks and wakes across tasks
and a task-limit rejection that retains the parent's right. Hand-built checked
Core now verifies and interprets reply creation, completion, receive, close,
and child transfer. Cranelift JIT and standalone object output now run these
checked Core paths, including pending receive, cancellation, end, and spawn
failure. Source programs now expose `reply<I64>()?`, `ReplyTo<I64>.complete`,
`ReplyReceiver<I64>.receive`, explicit close, and spawn transfer. Completion
returns `Complete<I64>.Delivered | Closed(value)`; receive returns
`Reply<I64>.Value(value) | End`. Source tests compare interpreter, JIT, and
standalone execution for delivery, cancellation, and lexical close. General
request payload shapes beyond `I64` remain open.
Hand-built checked Core now models the runtime's concrete `Request<I64>`
shape with explicit pack, unpack, request-channel, send, receive, and
structural close operations. Its verifier tracks the nested reply right as
one owned slot across each move and queue boundary; interpreter tests cover
delivery and rejected-send recovery across the interpreter, JIT, and standalone
executable. Source now has `request_channel<I64>(capacity)?`,
`request(value, right)`, request send and receive in exhaustive `when` arms,
`unpack()`, and explicit or lexical close. Source tests compare interpreter,
JIT, and standalone execution for delivery, closed-send recovery, cleanup,
resource limits, and a full request queue. The two-request, one-slot example
retains the second request's nested reply right across a pending send and
resumes after the first dequeue. Core now
computes static capability leaf paths for acyclic nominal
products and reserves one ownership slot per leaf in each SSA value. Core
flow moves all leaves through construction, nominal move, block arguments,
owned calls, spawn transfer, and whole-product unpack. The interpreter,
direct Cranelift path, retained task path, and standalone output agree on
nested closed-product unpack. Product declarations may contain channel,
reply, and concrete request rights. Source `product.close_caps();` consumes a nominal
product; closed products generate no native close calls or runtime context.
Core and ABI code enumerate nested capability leaves in reverse field order.
The abstract ownership graph now transfers all leaves on return and creates
all leaves in an owned call result; synthetic tests check that a caller must
dispose of the result. The Core verifier rejects copying, partial field
projection of a product with rights. Mutable places now accept nominal
capability products and sums. A take empties the place; storing a new owner
requires that empty state. Implicit `?`
cleanup emits one structural close per live product, and interpreter, JIT,
and standalone source tests cover nested close, owned call/return, whole
unpack, spawn transfer, and resource failure cleanup. Interpreter and JIT
tests also cover task-limit rejection before product transfer.
Source and Core tests cover take, reassignment, suspension, and failure-edge
cleanup of a live place. Bare rights in `var` remain unsupported. The
finite models and tests do not prove
correctness of the compiler or generated machine code.
The bounded two-request Maude model explores both successful retry and
receiver closure during a pending send. It checks encoded queue/right
ownership and terminal cleanup over 25 states; it is not a proof about the
generated frame or scheduler. See `formal/README.md`.
The runtime also has native ABI shims for this concrete request layout.
Native poll tests cover atomic creation, enqueue/dequeue transfer, closed-send
retention, and pending receive wakeup. Cranelift lowering uses these shims for
the concrete two-word request layout.
Runtime task failure now cancels remaining scope tasks immediately after
closing the failing task's rights. A host regression catches the case where
the parent retained another sender and would otherwise report a deadlock
instead of the child's fault. The bounded two-task Maude model includes
atomic scope cancellation and queued-message discard; it remains an abstract
model, not a proof about the executable scheduler.
Typed resource creation errors now distinguish invalid capacity, table
limits, and allocation failure without publishing partial endpoint pairs.
Task body boxing now fails before spawn transfers commit; an injected
allocation failure test checks intact child and channel-right recovery.
Uncontrolled host OOM, including error-string allocation, is not yet covered.
`docs/runtime-kernel.md` describes the kernel and the compiler,
general typed-message, and native-runtime integration still required.
`crates/capability` separately checks exact slot ownership in abstract
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
the `?` cleanup closes it. Children without suspension also run through the
Cranelift JIT and standalone object path.
Named nominal `var` arguments are taken before spawn. The success edge
transfers their rights; the failure edge closes the retained values. A source
test covers direct and suspended parents, task-limit rejection, and
reassignment of the emptied place after successful spawn.
Source `val another = sender.fork()?` and the receiver equivalent now create
one independently owned handle on success. The original remains live on both
edges, and a resource failure creates no new handle. Core checks the fresh
slot and requires the original right on each edge. Direct code, retained
tasks, the interpreter, and standalone binaries run the same fork operation;
`examples/fork_channel.phoebe` exercises both handle kinds. Allocation
failure is represented in the ABI but has not been forced in a test. A scope
now also bounds total channel handles minted (default 256); a handle-limit
failure is tested in the kernel, native ABI, interpreter, and JIT. Closed
tokens are not reused, so a close does not refund that budget.
Source `send` and `receive` now yield explicit `Send<I64>` and `Receive<I64>`
outcomes. A two-message source exchange over a one-slot channel exercises
parking and wakeup in the interpreter. The outcome types are added to Core
only when used. General source capability safety and cancellation cleanup
remain open. `docs/capability-ir.md` specifies
the integration boundary.
Source `with name = capability { ... }` provides lexical fall-through
cleanup for one bare right or a whole product that owns rights. The grouped form
`with (output = sender, input = receiver) { ... }` nests those regions for
several rights, including request channel endpoints. Exact capability flow
adds a close only when that right remains live. Explicit close or successful
transfer suppresses it; source `?` still closes live bare rights on its error
edge. Nested endpoint scopes run in the interpreter, JIT, and standalone
object path. A `with` body with a final expression can now return a
capability-free value through a Core block parameter; the endpoint closes
before that value leaves the block. `examples/with_result.phoebe` exercises
normal and `?` exits; `examples/with_wait.phoebe` retains the owner across
channel suspension before cleanup. `examples/with_pair.phoebe` covers grouped
rights on normal and `?` exits. `examples/with_product.phoebe` covers
structural product cleanup; source tests also cover nested aliases and spawn
transfer. General failure cleanup remains open.
Capability-bearing sums now use one exact ownership envelope per sum value.
`PackSum` consumes the selected payload rights, and `UnpackSum` exposes only
the matched variant's rights. Source `Option` and `Result` values may carry
bare rights or closed and capability-bearing nominal payloads. The ABI crate
computes one structural close plan with reverse product order and tag-aware
sum dispatch; the interpreter, direct Cranelift path, and retained path use
that disposition. Examples cover `Some`/`None` joins, a two-right payload,
owned calls and returns, lexical close after `yield`, and `?` cleanup of a
nested sum on resource failure. The standalone path agrees on those cases.
Copying these owners is rejected. A mutable place can hold the whole sum,
but must be emptied before reassignment. Spawn of a
capability-bearing sum now selects the active variant's physical rights for
the child, including sums nested in products. Direct and retained native
parents use a shared transfer emitter, and the runtime accepts more than four
descriptors. Source tests cover `Some`, `None`, five-right payloads, and failed
spawn cleanup in the interpreter and native paths. The bounded
`formal/sum_capabilities.maude` model checks one optional right moving to a
child only on successful admission; it is not a proof of the compiler or
runtime.
The versioned runtime ABI now has send and receive calls with explicit
`Pending` statuses and no endpoint transfer. Scheduler tests show wake and
retry across both operations. Generated send and receive leaves use retained
task frames. The native JIT and object paths use the same generated call
continuations and spawned poll children for the source exchange. A direct
parent can also spawn a suspendable child using a separate poll code object,
so the parent keeps its direct calling convention.
A suspendable function can call a direct-only helper with owned values and
receive a flattened nominal result without allocating a child frame; the
helper exchange runs in the interpreter, JIT, and standalone object path.
A suspendable parent can spawn a direct-only child through a poll-compatible
wrapper. An executable test parks the parent until that child closes a sender.
A retained parent can call a chain of direct helpers that spawns a suspendable
child. The poll module includes that child's entry, and interpreter, JIT, and
standalone tests cover the transfer and task-limit failure path. A standalone
poll child may also reach a direct helper that spawns another retained child.
Core now checks scalar send/receive terminators and derives live-right maps
for retry suspension. Its interpreter retains frames and places across a
pending operation, and executes buffered scalar traffic. Core `spawn` now
transfers endpoint and closed nominal arguments only on success. A two-task
Core program parks a parent receiver, wakes it from a child send, and handles
a task-limit failure without losing the parent's rights. Native retained
frames now execute the source exchange in the JIT and standalone object path. A Rust host test also spawns a
checked Core receiver that parks, wakes on a send, and returns its value.
Internal Core calls can transfer an endpoint to a callee that closes it;
interpreter and JIT executions agree on that synchronous path.
`phoebe frames` now prints the verified Core suspension plan. A backward
liveness pass finds only functions that can park and the values and places
needed at each channel retry or pending call. It leaves creation-only and
spawn-only functions on the direct-call path. `docs/native-frames.md` states
the frame ABI obligations and evidence boundary. The generated poll leaves
consume this analysis to produce exact retry spills.
The runtime now owns a stable word-array frame for a versioned native poll
entry. Synthetic poll functions exercise yield/resume and receive park/wake
through the real scheduler and ABI. A constrained Cranelift poll function
now executes checked source receive and two-send leaves through that same
scheduler. Tests cover value, end, backpressure, and closed-send branches.
The versioned spawn ABI initializes a generated child frame before committing
capability transfer; a task-limit test confirms the parent retains its rights.
Native source channel traffic now runs through the public `run` command,
including the two-message exchange and its task-limit failure edge. The
standalone `build` path links generated poll objects with the runtime archive;
executable tests cover the exchange and closed-send/end outcomes.
The compiler now assigns physical word ranges from the existing flattened
ABI shapes. A suspendable function reserves its result, entry parameters,
and values and places live across a pending operation. Entry parameters
already in the spill set share their slots. Stable per-function resume IDs
identify the retry site. The poll generator consumes these ranges for channel
retries and call continuations. A single reusable call-result area holds the
widest child result in a frame. Retained code generation now keeps mutable
places that cross suspension in frame words and uses local stack slots for
places that die before suspension. The accumulator example exercises this
and ordinary scalar and product expressions after resumption through
interpreter, JIT, and standalone executable paths. Retained calls now pass
borrowed places and forwarded references to suspendable callees. The caller
frame owns the borrowed place until the child returns; tests cover exclusive
and shared borrows across receive. General reference regions remain open.
Backend independent transport shapes now live in `crates/abi`; native lowering
and frame word counts use those same shapes. Equal shapes do not yet imply a
shared generic body or capability behavior.
The bounded Maude retained-borrow model checks one nested call chain with
park, wake, return, and ordered failure unwind. It does not prove the Rust
implementation or generated code.
The runtime has a tested same-task native call mailbox and child frame stack;
generated Cranelift calls use that continuation ABI.
Source `yield;` is an explicit cooperative checkpoint. Core records a typed
resume edge and a source-spanned `task.yield` cost site. The interpreter and
native poll backend requeue the task, retaining live places and capabilities;
the standalone build uses the same runtime ABI. `examples/yield_loop.phoebe`
has 42 executed explicit yields from one static site. A cheap source call
graph connects direct calls and spawned children. Functions in a component
containing `spawn` receive a loop-local countdown for each `while` and an
automatic checkpoint after each 1,024 completed backedges. Loops outside
those components keep the direct path without checkpoint bookkeeping.
The cost report distinguishes the automatic site from an explicit `yield`;
the Core IR shows the counter and resume edge. A 2,500-iteration loop in a
spawning program executes two automatic yields in the native scheduler,
and interpreter, JIT, and standalone results agree. A two-child test also
shows a quick sender overtaking a CPU-bound sibling after its checkpoint.
This bounds uninterrupted iterations of a `while` body, but does not bound
the work inside one body or
prove cancellation latency in wall-clock time. A shared helper can connect
otherwise separate entries in this conservative call-graph test.
`docs/performance-baseline.md` measures the cost on a local standalone loop.

## Commands

```sh
buck2 build //:phoebe_dist
buck2 run //:phoebe -- run examples/hello.phoebe
buck2 run //:phoebe -- run examples/branch.phoebe
buck2 run //:phoebe -- run examples/loop.phoebe
buck2 run //:phoebe -- run examples/borrow.phoebe
buck2 run //:phoebe -- run examples/variant.phoebe
buck2 run //:phoebe -- run examples/product.phoebe
buck2 run //:phoebe -- run examples/product_unpack.phoebe
buck2 run //:phoebe -- run examples/payload.phoebe
buck2 run //:phoebe -- run examples/try.phoebe
buck2 run //:phoebe -- run examples/nested_result.phoebe
buck2 run //:phoebe -- run examples/option.phoebe
buck2 run //:phoebe -- run examples/nested_variant.phoebe
buck2 run //:phoebe -- run examples/unit_tasks.phoebe
buck2 run //:phoebe -- run examples/unit_data.phoebe
buck2 run //:phoebe -- run examples/copy.phoebe
buck2 run //:phoebe -- run examples/channel_create.phoebe
buck2 run //:phoebe -- run examples/channel_cleanup.phoebe
buck2 run //:phoebe -- run examples/spawn_close.phoebe --interp
buck2 run //:phoebe -- run examples/channel_exchange.phoebe --interp
buck2 run //:phoebe -- run examples/channel_exchange.phoebe
buck2 run //:phoebe -- run examples/reply_exchange.phoebe
buck2 run //:phoebe -- run examples/request_exchange.phoebe
buck2 run //:phoebe -- run examples/request_backpressure.phoebe
buck2 run //:phoebe -- frames examples/channel_exchange.phoebe
buck2 run //:phoebe -- cost examples/copy.phoebe
buck2 run //:phoebe -- ir examples/hello.phoebe
buck2 run //:phoebe -- build examples/borrow.phoebe -o /tmp/phoebe-borrow
/tmp/phoebe-borrow
buck2 test //...
python3 formal/check_models.py
```
