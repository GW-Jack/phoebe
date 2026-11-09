# Typed message transfer contract

Status: concrete `channel<T>` messages run in the interpreter, Cranelift JIT,
and standalone code for the admitted scalar, nominal, sum, and `ReplyTo<I64>`
subset. The executable kernel has a scalar `I64` queue and a preallocated
fixed-width queue that carries `Request<I64>` and other messages containing
`ReplyTo<I64>` rights. This document defines the bridge from checked Core
values to a general bounded queue. Transfer of other right kinds and a general
proof remain open.

## Message admissibility

`T` is sendable when its entire value is closed or consists of closed fields
and movable communication rights. A task-local reference, place, loan, scope
token, or task frame is never sendable, even if its flattened ABI happens to
look like a word. The type checker decides sendability recursively. The Core
verifier checks the resulting typed operation and exact disposition of each
owned slot. Runtime shape validation is a defense against compiler faults,
not a substitute for either check.

`ReplyReceiver<I64>` is excluded from the first descriptor plan because its
receiver remains tied to the creating scope. Other communication rights still
need runtime transfer rules and scope checks before source channels accept
them as messages.

For a concrete `T`, the compiler builds one immutable message descriptor:

* `shape`: the flattened ABI parts and their alignment;
* `close`: a structural, tag-aware plan for active capability leaves;
* `transfer`: the same active leaves with their runtime capability kinds;
* `type_id`: an identity for the complete concrete type, not merely its ABI
  shape. Equal shapes may have different ownership behavior.

The descriptor is shared by the channel's two endpoints and all its slots.
It contains no task-owned values. A sum's tag selects exactly one arm before
its payload is inspected. Padding and inactive arms never carry a right.
`Request<I64>` is a two-word first example: payload, then `ReplyTo<I64>`.
`phoebe_layout::Shapes` now computes a concrete type, shape, and close-plan
layout for eligible messages. `MessageLayout::visit_active_rights` walks the
selected leaves without allocating on success, validates the ABI word count,
and rejects an out-of-range active tag. The runtime has an independent
`TypedRightPlan` for `ReplyTo<I64>` leaves, nested sequences, and tagged arms.
`phoebe_message::MessageCatalog` builds one shape table for a checked program;
its `runtime_message_descriptor` translates the checked ABI plan
to that runtime plan, preserving flattened offsets and structural close order.
Its type ID is unique within one linked program and must not be reused across
independently linked programs. The interpreter's concrete `Request<I64>`
channel now creates its runtime queue from this catalog and encodes and
decodes its two words through the same layout table. Checked Core channel
operations and generated native descriptor calls use the same catalog for
admitted concrete messages. Transfer of other right kinds remains open.
An executable aggregate witness encodes a tagged product with an active reply
right, transfers it through the runtime queue, decodes it, and closes the
returned right. Source regressions now carry both active and inactive tagged
reply-right arms through generated native channels.
Checked Core now has `TypedChannelNew`, `TypedSend`, and `TypedReceive` for
concrete message identities. The verifier checks exact endpoint and message
types and derives capability actions for success, closed, retry, and value
edges. Hand-built Core programs interpret nominal messages, including a
product that owns a reply right. Source `channel<T>` lowers concrete nominal
traffic and a closed send that returns an owned reply right. Native lowering
passes the checked descriptor identity and flattened words to the versioned
runtime ABI.

## Ownership equations

Let `A(T, m)` be the multiset of active capability identities in message `m`
according to its descriptor and tag. Let `Task(t)`, `Queue(q)`, and `Closed`
be disjoint ownership domains. A message is *intact* when each member of
`A(T, m)` is owned by the same domain as the message. A channel slot contains
one complete message or is empty. No transition may expose a partially moved
message.

| Event | Before | After |
| --- | --- | --- |
| `Sent` | `m` and `A(T,m)` owned by sender task | Both owned by one queue slot |
| `Value` | `m` and `A(T,m)` owned by queue slot | Both owned by receiver task |
| `Closed(m)` | Both owned by sender task | Unchanged; return intact `m` |
| `Pending(m)` | Both owned by sender frame | Unchanged; retry with same `m` |
| precommit resource failure | Both owned by sender frame | Unchanged; return intact `m` |
| last receiver closes | Both owned by queue slot | Close active rights in structural reverse order, then empty slot |
| task cancellation/failure | Frame owns `m` | Close active rights through the frame's live-slot map |

The runtime must validate all active rights and reserve any required storage
*before* publishing a queue slot or changing a right's owner. Once the first
owner changes, the remaining changes and slot publication are one
nonfailing commit phase. A rejected send and a full queue never cross that
point. A dequeue changes the active rights to the receiving task before it
returns the value. Queue discard uses the descriptor's `close` plan; it does
not invoke user code or a general garbage collector.

## Representation and cost constraints

Channel creation reserves exactly `capacity` message slots or returns a
resource error with no published endpoints. A send into available capacity
must not allocate. Fixed-size concrete messages fit inline in those slots;
the descriptor determines slot width and alignment. A maximum slot width or
total byte budget may be imposed as a diagnosed resource limit. The compiler
must not silently box large messages or introduce per-send refcounting.
Waiting sender frames retain their original value; the queue does not copy a
pending message. A successful send copies the closed bytes once and moves
only the active rights. For an empty `Unit` message, the queue still records
occupancy so boundedness is preserved.

The native typed queue C ABI now accepts a concrete type ID, flattened width,
and a versioned structural right plan at creation. The plan is a preorder word
tree: version `1`, then `Empty = [0]`, `ReplyTo(part) = [1, part]`,
`Sequence(children) = [2, count, children...]`, or
`Tagged(tag, arms) = [3, tag, count, arms...]`. The runtime validates the full
encoding before publishing either endpoint. Native send and receive shims pass
the same type ID and a contiguous word slice; success moves active rights,
while pending and closed sends retain the caller's words. Generated Cranelift
calls now pass the checked descriptor and flattened words through these shims
for concrete admitted messages. Zero-width messages pass a null word pointer
and still occupy a queue slot.

## Implementation gates

1. Keep the backend-independent `MessageLayout` in `phoebe_layout` as the source
   for runtime descriptor generation. Core and `phoebe_abi` can both use this
   crate without a dependency cycle. `phoebe_message` owns conversion to the
   runtime descriptor and is usable by both Core and native code. The layout
   tests cover nested products, `Request<I64>`, tagged sums, references, and
   recursive layout rejection. Native integration tests cover
   equal-width type distinction, active-arm offsets, and a tagged runtime
   transfer. The interpreter's concrete request and checked typed channel
   creation paths request it; generated native creation uses the same
   descriptor. Source regressions compare nominal, boolean-field, zero-width,
   reply-right aggregate, and tagged messages across interpreter, JIT, and
   standalone.
2. Extend the runtime's preallocated typed slots beyond `ReplyTo<I64>` rights.
   The current path checks type identity and width, validates all active reply
   rights before changing any owner, transfers multiple rights on enqueue and
   dequeue, closes them on discard, and preserves unsent rights on full and
   closed sends. Tests exercise active and inactive tagged arms, duplicate or
   invalid rights, and a two-task full queue. A dedicated allocation-counting
   test checks zero heap allocations for successful typed send and receive
   after channel creation. The same test injects waiter-allocation failure on
   a full queue and verifies that the queued right and unsent right survive
   until receive and retry. Transfer of channel handles remains open.
3. Complete source `channel<T>` and native code generation through the checked
   typed Core family. The interpreter, Cranelift JIT, and standalone paths now
   handle concrete nominal and reply-right aggregate messages. The existing
   concrete `Request<I64>` operations also run through typed runtime adapters.
   The native ABI passes descriptor identity and flattened words, never
   treating a matching word count as type identity. Transfer of channel
   handles and other right kinds is still outside this subset.
4. The bounded `formal/tagged_queue.maude` model covers a two-message FIFO
   with different active tags and reply-right leaves. It checks atomic
   transfer, FIFO receive, discard, and closed-send retention. Its finite
   search is design evidence; no refinement proof connects it to Core,
   runtime, or generated code. Independent executable tests cover tagged
   source traffic and runtime rights.
5. Compare interpreter, JIT, and standalone runs for the remaining
   recoverable/failure edges, then measure allocation count and send cost.

The release gate is met only when these transitions work for the documented
message subset, including a capability-bearing aggregate beyond the current
concrete request shape. The existing finite product and sum models in
`formal/` are useful obligations, not a proof of this integration.
