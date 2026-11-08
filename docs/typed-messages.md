# Typed message transfer contract

Status: design obligation for general `channel<T>` messages. The executable
kernel currently has separate `I64` and `Request<I64>` queues. This document
defines the bridge from checked Core values to a general bounded queue; it
does not claim that bridge is implemented or proved.

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
`phoebe_abi::Shapes` now computes a concrete type, shape, and close-plan
layout for eligible messages. `MessageLayout::visit_active_rights` walks the
selected leaves without allocating on success, validates the ABI word count,
and rejects an out-of-range active tag. The runtime descriptor and ownership
transfer implementation remain open.

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

## Implementation gates

1. Keep the backend-independent `MessageLayout` in `phoebe_abi` as the source
   for runtime descriptor generation. Its current tests cover nested
   products, `Request<I64>`, tagged sums, references, and recursive layout
   rejection. The runtime descriptor must preserve its type identity and
   active-leaf selection.
2. Add runtime-owned, preallocated typed slots and an atomic transfer API.
   Keep the existing scalar/request API as adapters while validating the new
   protocol. Test full, closed, dequeue, discard, and injected precommit
   allocation failure with multiple rights and active/inactive sum arms.
3. Add one checked Core typed send/receive family and lower source
   `channel<T>` through it. The native ABI passes descriptor identity and
   flattened words; it never treats a matching word count as type identity.
4. Extend the bounded Maude model to cover a two-message queue containing
   differently tagged aggregates. Its finite search is design evidence; an
   explicit refinement argument or independent executable tests are still
   needed for compiler and runtime behavior.
5. Compare interpreter, JIT, and standalone runs for success and every
   recoverable/failure edge, then measure allocation count and send cost.

The release gate is met only when these transitions work for the documented
message subset, including a capability-bearing aggregate beyond the current
concrete request shape. The existing finite product and sum models in
`formal/` are useful obligations, not a proof of this integration.
