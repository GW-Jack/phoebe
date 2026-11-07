# Capability flow in Core IR

Status: design contract and preparatory executable flow checker. The checker
in `src/capability_flow.rs` verifies exact capability-slot states at joins and
normal returns, and derives live-slot maps for failure and suspension edges.
The executable Core IR does not yet produce these graphs or contain these
types and operations. The runtime has a bounded `I64` request/reply
implementation, but it is not connected to source code.

## Why the current affine rule is insufficient

The present ownership verifier tracks live nominal values and intersects
predecessor states at a CFG join. That is appropriate for closed values: an
unused value can be dropped. It is not sufficient for a communication
capability. If a `ReplyTo` is consumed on one branch and remains live on the
other, intersection marks it absent after the join, thereby hiding the live
right on one path. A capability-bearing type must be *accounted for* on every
path, even if the source never names it again.

The compiler will classify every type recursively as:

| Class | Examples | Implicit discard |
| --- | --- | --- |
| Copy | `I64`, `Bool` | Yes |
| Closed affine | Current nominal sums/products of closed values | Yes |
| Capability-bearing | `Sender<T>`, `Receiver<T>`, `ReplyTo<T>`, `ReplyReceiver<T>`, aggregates containing them | No |

`copy(x)` is rejected for a capability-bearing value. `fork` is a distinct
operation for handle kinds that support it; it creates another tracked right
and has a runtime limit/failure edge. A `ReplyTo` has no `fork` operation.

## Accounting invariant

For a program point `p`, let `O(p)` be the set of live capability *slots* in
SSA values and task-local places. A slot identifies a structural path into a
value, such as `request.reply`; it is not a runtime pointer. Every live slot
has exactly one owner: the running task, a retained task frame, a channel
queue, or another task. The compiler tracks the first two; the runtime owns
the queue/task handoff. A transition may move, close, or return a slot, but
may not duplicate or forget it.

For every CFG edge `e: b -> c`, normalize the outgoing slots through its
block-argument mapping. The verifier requires the same capability ownership
state from every predecessor of `c`. If states differ, the lowering must
insert an explicit close or transfer on the appropriate predecessor edge.
Closed affine values retain the existing intersection rule. The rule applies
to loop backedges too; a loop cannot accumulate an unaccounted right.

There is no runtime scan corresponding to this rule. A compiler verifier
checks the finite IR once per changed function. Its worklist propagates
compact bitsets of capability slots; ordinary generated code executes only
the explicit transitions. Maude explores bounded abstract machines during
development and CI; it is not invoked for each compilation.

## Operations and edges

The following is the intended checked Core vocabulary. `T` must be sendable
for a channel, and a queue's capacity is established when it is created.
Resource creation has a typed failure edge rather than an allocation panic.
Because creation can fail, `channel.new` and `reply.new` are terminators. Their
success successors receive two fresh capability-bearing block parameters;
their failure successors receive a small resource error and no new capability.
Ordinary instructions retain zero or one result. The Core representation
must model the two creation outputs on the success edge, rather than pay for
multi-result storage on every instruction.

For creation from live state `O`, the verifier checks these edge equations:

```text
channel.new success: O' = O ∪ {sender, receiver}
channel.new failure: O' = O
reply.new success:   O' = O ∪ {reply_to, reply_receiver}
reply.new failure:   O' = O
```

The fresh slots exist only on the success successor. They must be distinct,
and the runtime must publish both rights or neither. A normal return from
either successor still requires exact disposition of every live right. The
same edge form handles `spawn_owned`: success transfers the child frame's
slots to the new task; failure returns the intact frame and retains its slots
with the parent.

| Operation | Required task ownership | Success transition | Other edge |
| --- | --- | --- | --- |
| `scope.enter` | Parent scope live | Create child scope budget | Resource failure |
| `channel.new<T>(scope, n)` | Scope live; `n > 0` | Create one sender and one receiver | Budget/allocation failure; no partial handles |
| `handle.fork(h)` | Own `h`; kind permits fork | Retain `h`, create new owned handle | Budget/allocation failure; retain `h` |
| `handle.close(h)` | Own `h` | Consume `h`; runtime wakes peers/discards queued messages as needed | None |
| `reply.new<T>(scope)` | Scope live | Create `ReplyTo<T>` and `ReplyReceiver<T>` | Budget/allocation failure; no partial rights |
| `reply.complete(right, value)` | Own both | Consume right; deliver value | Receiver closed: consume right, return intact value |
| `reply.cancel(receiver)` | Own receiver | Consume receiver; discard buffered value | None |
| `reply.receive(receiver)` | Own receiver | Value: consume receiver, return value | Pending: retain receiver in frame; End: consume receiver |

`channel.send(sender, message)` is a **control-flow terminator**, not an
unconditional consuming instruction. Its `Sent` edge retains the sender and
transfers all message capability slots to the queue. Its `Closed(message)`
edge retains both sender and the intact message. Its `Pending` edge retains
both in the task frame and suspends; on wake the same operation is retried.
An allocation or budget failure before enqueue likewise retains the message.

`channel.receive(receiver)` has `Value(message)`, `End`, and `Pending` edges.
The value edge retains the receiver and transfers message slots from queue
to task. The end edge retains the receiver until an explicit close. The
pending edge retains it in the frame. A successful dequeue wakes one blocked
sender. Source `?`, `when`, cancellation, and failure must preserve the same
edge accounting.

The runtime kernel's `spawn_owned` and `send_request_owned` now preserve the
child frame or request on pre-commit failure. A Core lowering must use these
recoverable paths so its failure edge can retain or explicitly close the
returned capability slots. The Rust convenience wrappers that reduce an
error to a string are suitable only for immediate task failure and cleanup.

The scheduler owns a pending task's frame, so suspension does not release any
of its rights. The generated frame has a fixed layout and a compiler-derived
cleanup table listing every possible capability slot. A discriminant records
which slots are live at each suspension state. On task failure or scope
cancellation, runtime cleanup closes exactly those live slots without running
user code. A queue slot has its own structural cleanup entry; dropping a
queued request closes its `ReplyTo` and wakes a waiting reply receiver.

## Exit rule

Every normal return and early `?` edge must leave no task-owned capabilities
other than those explicitly moved to its result or parent. A scope-local
`ReplyTo` cannot escape its creating scope. A `with` boundary inserts
structural close operations on all normal exits for capabilities still owned
there. Replacing a capability-bearing place first closes or transfers its
old content. A match arm that discards a capability-bearing payload must
close it explicitly. The verifier rejects a missing close, including one
hidden by a branch join.

Failure is a separate terminator: it transfers the frame's live-slot bitmap
to runtime cleanup. It is not a normal return with ignored capabilities.
Cleanup itself is a small, compiler-generated operation table shared by the
interpreter and native runtime ABI. This keeps ownership decisions visible
in printable IR and avoids a second, implicit compiler cleanup algorithm.

## Verification boundary and implementation order

The preparatory flow checker rejects implicit capability loss in an abstract
graph, including differing branch states, loop backedges, duplicate creation,
and normal return. It does not yet classify Core types, derive graphs from
Core, or check place replacement and match discard. Next, add that type and
graph integration, lower `with` and structural cleanup, then add the
send/receive/reply terminators and retained frame layout. Interpreter and
Cranelift executions must agree on the same IR, including failed and canceled
paths. Only after those steps should source syntax expose the operations.

The verifier establishes a property of the checked IR, conditional on its
implementation and runtime ABI. It does not prove that Rust cleanup, Cranelift
lowering, or source lowering preserves that property. The bounded Maude model
in `formal/reply_capability.maude` checks a smaller request/reply machine; it
is evidence for that model alone.
