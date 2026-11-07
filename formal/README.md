# Bounded formal models

Run `python3 formal/check_models.py` from the repository root. Maude 3.5.1 was
used for the initial checks. The script rejects warnings and unexpected search
results because Maude can exit successfully after a malformed command.

## One-task core

`core.maude` has one task, two nested regions, one reference rooted in the
inner region, one sender, one receiver, and a one-slot queue containing only a
closed integer value. It checks that a live reference cannot survive its
region, and that task failure closes handles and storage. The initial search
explored 64 reachable states with no encoded bad state. A separate sanity
query confirms that a deliberately dangling reference is classified as bad.

## Two-task channel model

`two_task.maude` starts with a parent holding sender and receiver handle 0.
`fork` creates handle 1 with parent ownership. `spawn` transfers both handle-1
capabilities to the child. Each task can send at most one distinct closed
value; a one-slot queue carries either value. A message disposition is `new`,
`pending`, `delivered`, `discarded` after all receivers close, or `rejected`
when a send observes no receiver. The model allows all enabled actions in any
order. Child failure closes its handles and cancels the parent before another
task poll. Cancellation closes remaining rights and discards a queued message;
the scope reports the child fault.

The bad-state predicate checks queue/disposition agreement, handle ownership
relative to task status, and scope completion status. The initial search
explored 340 reachable states with no encoded bad state. Queries also witness
spawn, cross-task delivery, send rejection, explicit discard, normal scope
completion, and failure reporting. A negative query confirms a closed sender
cannot be forked back into existence.

## One-shot reply capability model

`reply_capability.maude` has one parent/client, one service child, one request
slot, one reply slot, and one `ReplyTo` token. The token has exactly one owner:
parent, request queue, child, or closed. Its rules move the token on send and
receive, consume it on completion, and close it on abandoned replies, queued
request discard, or child failure. Cancellation either rejects a later
completion or discards an already queued reply. A scope joins only after the
request sender, child, reply token, and reply slot are resolved.

The bad-state predicate checks queue/token agreement, completion and receiver
states, child cleanup, and scope closure. The exhaustive search explored 88
reachable states with no encoded bad state. Ten positive queries witness
ownership transfer, successful and canceled completion, discard, failure,
and both scope outcomes. A deliberately inconsistent state is classified bad.

## Retained borrow through a nested call

`retained_borrow.maude` models one parent-owned scalar place borrowed by a
middle function and forwarded to a child that can park. The parent and middle
frames stay live while the child holds the reference. A successful path wakes,
writes through the reference, releases the child and middle loans, and frees
the parent frame. Failure can begin while the child runs or parks; it unwinds
the two loans in order before freeing the parent frame. The search explores
18 states with no encoded dangling reference or premature frame release.
Four witnesses cover parking, writing, normal return, and cleanup after a
parked failure. A deliberately freed parent with active loans is classified
bad.

## Evidence boundary

These are finite explorations of four handwritten abstract machines. The
two-task model does not include regions or references; the one-task model
does not include handle fork. Neither models multiple queued messages or
blocking/parking. The reply model does not cover multiple reply rights,
general transferable handles, multiple queued requests, or memory regions.
The retained borrow model includes parking but only one same-task call chain,
one scalar place, and an abstract ordered unwind. It does not encode the
generated frame offsets, Rust pointer operations, or allocator behavior.
None models scheduling fairness, foreign calls, or executable compiler IR.
No proof yet connects any model to the compiler, runtime, or generated machine
code. A `No solution.` result means only that its stated bad predicate is
unreachable under its stated rules.
