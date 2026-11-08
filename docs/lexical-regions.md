# Lexical borrow regions

Status: design obligation for borrowing beyond one call. Current executable
Core supports only call-scoped I64 and Bool loans. The finite
`formal/lexical_borrows.maude` model checks a two-level instance of the rules
below; it is not a proof of Core or native code.

## Ownership and authority

Every loan has a stable task-local storage root, a mode (shared or exclusive),
and a lexical region. Its region must end before the storage root is released:
`lifetime(loan) ⊆ lifetime(root)`. A move, drop, or replacement of that root
requires no live loan. A shared loan permits reads and further shared loans.
An exclusive loan permits reads and writes. Reborrowing an exclusive loan
freezes the parent authority until the child loan ends; borrowing through a
shared loan cannot create exclusive authority. A reference never owns its
root and cannot cross a task or channel boundary.

On normal exit, nested loans end in reverse region order before the root is
released or transferred. On task failure, unwind uses the same order before
freeing a frame. Suspension retains the root at a stable frame address and
preserves its loan state; resumption does not create a new loan. Nominal
products and sums may contain owned capabilities, but cannot hide references
whose region would outlive the aggregate. Core already rejects reference
fields and payloads.

## Proposed Core boundary

Extend the existing `LoanRegion` from a call-only record to a tree of lexical
regions. Each `LoanBegin` names a root place or an existing loan, its mode,
and its parent region; each `LoanEnd` consumes that authority. The verifier
should run one forward dataflow over a finite state per root: available,
shared-count, exclusive, or frozen-by-child. CFG joins require compatible
authority states, and loop backedges must reach a fixed point. Ordinary
ownership and capability-flow verification remain separate passes over the
same explicit Core edges. The native frame plan must prove that every root
live at a suspension point has a stable slot and that cancellation releases
loans before that slot. These checks should be linear in Core size apart from
the existing CFG fixed point, with no optimization search during compilation.

Before adding source-level nominal or field borrows, implement these Core
transitions, forged-IR rejection tests, and interpreter/native differential
cases for nested shared loans, exclusive reborrows, moves after region exit,
suspension, and failure. The Maude model is backpressure on the transition
design, not a substitute for those executable checks.
