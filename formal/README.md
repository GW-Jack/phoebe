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
order. Child failure closes its handles; parent may drain a pending message,
then join the scope and observe failure.

The bad-state predicate checks queue/disposition agreement, handle ownership
relative to task status, and scope completion status. The initial search
explored 398 reachable states with no encoded bad state. Queries also witness
spawn, cross-task delivery, send rejection, explicit discard, normal scope
completion, and failure reporting. A negative query confirms a closed sender
cannot be forked back into existence.

## Evidence boundary

These are finite explorations of two handwritten abstract machines. The
two-task model does not include regions or references; the one-task model
does not include handle fork. Neither models multiple queued messages,
blocking/parking, scheduling fairness, foreign calls, or executable compiler
IR. In particular, no proof yet connects either model to the proposed
compiler, runtime, or generated machine code. A `No solution.` result means
only that its stated bad predicate is unreachable under its stated rules.
