# Executable Core IR: current subset

Status: implementation note for the compiler under development. The current
IR is not yet Phoebe's complete checked IR or a proven semantics.

A function contains typed SSA values, numbered basic blocks, and numbered
mutable places. Each block has zero or more typed parameters, an instruction
sequence, and exactly one `return`, `jump`, `branch`, `match`,
`channel.new.i64`, `channel.send.i64`, `channel.receive.i64`,
`reply.new.i64`, `reply.complete.i64`, `reply.receive.i64`, `spawn`, or
`scope.join`
terminator. A `jump`
supplies one typed value per destination parameter. An `if` expression lowers
to a conditional branch and a join block whose parameter carries the chosen
arm's result. A `while` statement lowers to a loop head, body, exit, and
backedge.

Source `scope { ... }` and hand-built Core enter a lexical task scope with `scope.enter r` and
leave it through a suspending `scope.join r -> resume` terminator. The verifier
tracks the exact stack of open region IDs at each block, requires LIFO joins,
and rejects a return while any scope is open. The interpreter, retained
Cranelift JIT, and standalone object path execute an enter, spawned child,
pending join, and resumed return. Source lowering also routes implicit `?`
returns through joins from inner to outer; see `docs/scope-ir.md`.

Nominal sum variants have a distinct `Data(id)` type and an explicit `variant`
instruction. Source `when` checks every variant exactly once, then lowers to
one `match` terminator with a target per variant. Each edge explicitly binds
the active payload, the whole subject, or nothing. The verifier checks the tag
bounds and the edge binding against its target's parameter type and count.
Payload access therefore
does not need a separate unchecked field operation or path-sensitive proof
search. Native lowering expands the match into a tag dispatch; the interpreter
selects the matching arm directly. Finite products have `product` and
`field` operations. Their fields remain separate SSA components through native
calls, returns, and joins; `docs/abi-shapes.md` specifies the current shape.
`product.unpack` consumes a whole nominal product and passes one typed block
parameter per field. Nested fields retain their flattened ABI shape. The
interpreter, direct Cranelift path, retained task path, and standalone path
agree on a nested closed-product example. Source `val (a, b, ...) =
product.unpack();` lowers to this terminator and requires one name per field.
The existing two-name request unpack uses the same binding syntax.
Capability-bearing product declarations are accepted. The verifier requires
whole-product unpack for field ownership, rejects copying or mutable place
storage of such a product. Capability-bearing sums use one affine envelope
slot; variant construction packs the selected payload rights into it, and
`match` unpacks only the active arm. `close.caps` consumes a nominal product
or sum. Products close fields in reverse order; sums dispatch on the tag and
close only the active payload. A closed nominal value has no runtime close
effect. Source `value.close_caps();` emits this operation. The verifier rejects
reuse of the consumed owner, and native code emits no close call when the
value has no rights. The interpreter, JIT, and standalone paths run nested
capability products through close, owned calls, returns, and spawn transfer;
`examples/capability_product.phoebe` demonstrates the source form.

The first `?` lowering uses this same `match` terminator. Its `Err` target
receives and returns the original sum value through a block parameter; its
`Ok` target binds the payload. There is no new `Err` construction. See
`docs/result-try.md` for the executable rule and missing cleanup obligation.

`val` binds an SSA value. `var` creates a task-local place. The current
place operations are `place.init`, `place.load`, `place.take`, `place.field`,
and `place.store`. An
initializer dominates every use, including on a loop backedge. A place can
hold `I64`, `Bool`, `Unit`, sums, or finite products. A synchronous call can
borrow an I64 or Bool place through an explicit shared or exclusive argument;
the callee uses `ref.load` and, for an exclusive loan,
`ref.store`. The interpreter stores a value per place and resolves the call
borrow to the caller's slot. Cranelift lowers places to typed stack slots and
passes their addresses. This separation keeps reference and region rules
attached to *places*, rather than pretending a mutable object is an SSA value.
Hand-built Core can open, reborrow, and close a scalar lexical loan with
`borrow.begin`, `borrow.reborrow`, and `borrow.end`; its verifier tracks active
and frozen loans across CFG edges. A live loan may cross `yield` with a stable
root in the retained frame. Source lowering still emits only call-scoped loans. See
`docs/lexical-regions.md` for the implemented boundary and open rules.

Nominal values are currently affine. Reading a nominal `val` for an owning
use emits `move`; reading a nominal `var` emits `place.take`. A primitive
`copy(x)` emits `copy.value` for a nominal owner and preserves the source.
That IR operation is also an on-demand static cost event. A primitive
product field read uses `field` on a live SSA owner or `place.field` on a live
place, leaving the owner available. A nominal product field uses an owning
`field` projection; a `var` first emits `place.take`. The other closed fields
are discarded. The ownership verifier propagates live
value and place states through the CFG, consumes them at owning calls,
constructions, jumps, returns, and matches, and intersects them at joins.
Using an owner after a move receives a source-located diagnostic. The native
`move` reuses the same scalar components; `place.field` loads only the selected
primitive component. The interpreter's boxed values remain a reference implementation,
not a native allocation model. `affine-ownership.md` gives the abstract
transition rules and their evidence boundary.

The structural verifier checks unique SSA definitions, operand dominance,
block reachability, parameter, call, and return types, branch targets, call modes,
one dominating initializer per place, and the restricted borrow rules in
`call-borrows.md`. A borrowed call also has a `LoanRegion` record naming its
call instruction, place or forwarded-reference roots, and the call
return/unwind end. The verifier requires exactly one matching record for each
borrowed call. Frame analysis checks that a place root remains in the parent
frame when the callee can suspend. Source lowering verifies the final graph
after cleanup insertion and returns an immutable `CheckedProgram` to the CLI.
Its borrowed `VerifiedProgram` token carries that guarantee through direct,
retained, object, and cache paths without rerunning the verifier for each
native child. Public APIs accepting a mutable, hand-built `Program` verify it
before code generation. The token is an engineering boundary, not a proof
that lowering is correct. Differential tests compare interpreter and JIT results on supported
programs. The Maude models cover different bounded abstract machines; they
do not model this executable IR.

The current nominal-value check is affine: unused closed values may be dropped.
Replacing an owned `var` currently discards the old closed value, and `match`
may discard a payload. The communication capability check has exact
disposition at return, but nested capabilities still need structural cleanup
or explicit transfer on those exits.
General loan lifetimes beyond one call, lexical region begin/end, aggregate
cleanup edges, and the remaining cost event kinds are still open. The verifier will need to check those
without turning ordinary compilation into proof search.
`capability-ir.md` gives the intended edge accounting and suspension rules
for the next implementation slice.

`channel.new.i64` is a checked Core edge. It takes an `I64`
capacity and has a success successor with `SenderI64` and `ReceiverI64` block
parameters, plus a failure successor with a `ResourceError` parameter.
`handle.fork` takes an owned sender or receiver without consuming it. Its
success successor receives one fresh endpoint of the same type; its failure
successor receives `ResourceError` and no new endpoint. The exact capability
verifier requires the original right on both edges and creates the new slot
only on success.
`close.handle` consumes either endpoint. Core derives a capability-flow graph
from these operations: only the success edge creates the two rights, jumps
move rights to destination parameters, and normal return requires both rights
to be closed. This is exercised with hand-built Core and source programs. The interpreter
runs creation and close through a scheduler with explicit scope limits,
including resource-failure edges. `interpret_with_limits` can vary those
limits; plain `interpret` permits 64 tasks and 64 channel creations. Cranelift
JIT and standalone object execution use the same scheduler through a hidden
context parameter and agree on the tested success and failure paths. Source
lowering supports a fallible `I64` channel pair binding and explicit endpoint
close. Core also checks scalar send and receive terminators. Their sent,
closed, value, and end edges retain the endpoint; a pending operation records
the live capability slots and retries at the same terminator. One interpreter
loop handles both pure functions and channel tasks. Pure entries run it
directly; channel entries retain call frames and places across scheduler polls. A
buffered send/receive and an empty receive that parks are exercised in tests.
Native lowering runs these channel terminators through retained task frames.
Internal Core functions can take
owned channel endpoint parameters; a synchronous call transfers that right
to the callee, which must close it before returning. The interpreter and JIT
agree on an endpoint-owning call. The interpreter has also resumed a Core
child receiver after a scheduler wake; child creation in that test is still
performed by the Rust host. Core now also has a fallible `spawn` terminator
with separate success and resource-failure successors. Its child accepts
owned scalar, closed nominal, or bare endpoint arguments. Success transfers
those owners to a new task; failure retains them with the parent. The Core
interpreter runs a parent and child exchanging an `I64` over a bounded channel.
Source `spawn worker(sender)?;` now lowers to this terminator and uses the
exact failure-edge live-slot map for parent cleanup. It accepts only owned
arguments and an `I64`- or `Unit`-returning child. Nominal arguments may be
named `val`s, moved from a named `var`, or built as temporary expressions.
Failure retains each argument value for parent cleanup. Native spawn and
structural cleanup run in the interpreter,
JIT, and standalone path. Source `sender.send(value)` returns
`Send<I64>.Sent` or `Send<I64>.Closed(value)`, while
`receiver.receive()` returns `Receive<I64>.Value(value)` or
`Receive<I64>.End`. Both lower to the checked Core terminators; the
interpreter runs a source two-message exchange over a one-slot channel.
The outcome sums enter Core only in programs that use them.
Hand-built checked Core now has `ReplyToI64` and `ReplyReceiverI64` capability
types. `reply.new.i64` creates both only on its success edge. Completion
consumes the right on both delivered and closed edges. Receiving a value or
end consumes the receiver; pending retains it for retry. `close.reply`
consumes either right. The exact capability-flow verifier checks these edges,
including child spawn transfer and failure retention. The interpreter runs a
parent that moves a reply right to a child and parks on the receiver. Cranelift
JIT and standalone binaries agree on delivered, canceled, ended, spawn, and
resource-failure paths in hand-built Core fixtures. Generated spawns use typed
capability descriptors for both channels and replies. Source lowering now
emits these operations for `reply<I64>()?`, `complete`, `receive`, `close`, and
owned spawn arguments. Lexical and early-error cleanup select `close.reply`
for reply capabilities. Source request messages carrying rights remain open.

Checked Core now also has a concrete `RequestI64` value containing one `I64`
and one `ReplyToI64`. `request.pack` moves the right into the request slot;
`request.unpack` moves it out. A request channel creates distinct sender and
receiver types. `channel.send.request.i64` transfers the request to the queue
only on `Sent`, retains it on `Pending`, and passes the intact owned request
to the `Closed` successor. Receive creates a request slot on `Value` and
retains the receiver on `Pending`; `close.request` closes the nested right.
The exact capability verifier checks these transitions with one dense slot
for the request's known single right. Hand-built Core tests across the
interpreter, JIT, and standalone executable cover
queue transfer, unpack and completion, closed-send recovery, a child service
that waits for a request and completes its parent's reply, and rejected
implicit loss. Source now exposes the concrete request through structural
`when` outcomes and explicit pack, unpack, and close operations. This concrete
layout does not establish the general nested aggregate
capability rule. The control-flow source surface is in
`docs/request-source-design.md`.
