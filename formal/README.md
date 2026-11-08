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

## Two-request backpressure model

`request_backpressure.maude` has one parent, one service, two requests, two
reply rights, and one queue slot. The second send first returns pending with
its right owned by the parent. Dequeue permits a retry to transfer that right
to the queue. The service may park waiting for the second request. A separate
branch closes the last receiver while the send is pending; the rejected send
retains its request until the parent closes it. The encoded bad predicate
checks queue/right coupling, right ownership during pending and rejection,
reply publication, and terminal cleanup. The exhaustive search visits 25
states with no encoded bad state. Seven witnesses cover pending send, retry
before and after first completion, pending receive, successful completion,
rejection, and cleanup after
rejection. A deliberately mismatched queue/right state is classified bad.

This model fixes the number of requests and rights at two and abstracts away
the runtime frame layout, scheduler fairness, channel allocation, and memory
regions. The source and native regression for the successful retry path is
`examples/request_backpressure.phoebe`.

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

## Lexical task scope membership

`lexical_scopes.maude` has one root task, an earlier sibling in the root
scope, one child in an inner scope, and one grandchild spawned by that child.
The model stores root and inner live counts independently of task status.
Its 69 reachable states have no encoded counter or phase violation. A second
unreachable search checks that the inner join cannot finish while the
grandchild is live. Seven witnesses cover the earlier sibling, transitive
counts, a pending join, child completion while the grandchild remains live,
an inner join while the sibling remains live, root completion, and fault
cancellation. A deliberately joined state with a live descendant is bad.

This fixes the task set and permits at most one nested region. It abstracts
the scheduler's ready queue, scope slot generations, frame allocation,
channels, and native ABI. The source tests exercise the corresponding
sibling-isolation and failure paths, but no refinement proof connects them
to this model.

## Evidence boundary

These are finite explorations of six handwritten abstract machines. The
two-task model does not include regions or references; the one-task model
does not include handle fork. Neither models multiple queued messages or
blocking/parking. The reply model does not cover multiple reply rights,
general transferable handles, multiple queued requests, or memory regions.
The backpressure model covers exactly two requests and one queue slot; it
does not model executable frame spills or every cancellation interleaving.
The retained borrow model includes parking but only one same-task call chain,
one scalar place, and an abstract ordered unwind. It does not encode the
generated frame offsets, Rust pointer operations, or allocator behavior.
None models scheduling fairness, foreign calls, or executable compiler IR.
No proof yet connects any model to the compiler, runtime, or generated machine
code. A `No solution.` result means only that its stated bad predicate is
unreachable under its stated rules.
