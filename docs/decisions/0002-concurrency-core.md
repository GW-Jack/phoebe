# Decision 0002: Scoped concurrency and transferable reply rights

Status: accepted, 2026-11-06. This fixes the design direction; precise syntax,
runtime representation, and a general proof remain future work.

## Decision

Scoped tasks and bounded channels are Phoebe's concurrency primitives. A
child's lifetime remains inside its parent scope. Channel messages may contain
closed values and linear communication capabilities transferred by move.
Ownership passes atomically from sender to queue to receiver. Rejected sends
return the unsent message to the caller. Queue discard and task failure close
their owned capabilities exactly once.

The first named transferable capability is `ReplyTo<T>`: a one-shot,
nonblocking reply right paired with a receiver in the creating scope. Its
`complete` operation consumes the right and reports delivery or receiver
cancellation. No arbitrary task-local reference becomes sendable.

Actors are an abstraction over a scoped task, private state, and a bounded
mailbox. They run handlers serially and may offer more convenient syntax for
long-lived services. Actor handlers need explicit behavior for full mailboxes;
they cannot park while promising run-to-completion. Phoebe does not require a
separate actor heap or actor garbage collector for this abstraction.

## Rationale

Scoped tasks express parallel work and sequential flows that suspend. Actors
make serial stateful services pleasant to write but do not solve late replies,
slow subscribers, state reuse, or read-heavy shared configuration. Moving a
reply right through a request message removes substantial routing code from
both surfaces without allowing references into another task's storage.

## Verification boundary

`formal/reply_capability.maude` explores one client, one child, one request
slot, one reply slot, and one `ReplyTo` token. It checks queue ownership,
single consumption, cancellation, discard/failure cleanup, and scope closure.
This finite model does not establish a theorem for arbitrary message shapes,
multiple capabilities, general channels, allocation, or the future compiler.
The specification's general sendability rule is a design obligation to prove
in a typed IR semantics.
