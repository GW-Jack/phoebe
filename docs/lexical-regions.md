# Lexical borrow regions

Status: executable scalar source and Core slice plus remaining design
obligations. Source supports call-scoped loans and scoped `with` bindings for
I64 and Bool places. Checked Core supports explicit `borrow.begin`,
`borrow.reborrow`, and `borrow.end` for task-local scalar places.
The finite `formal/lexical_borrows.maude` model checks a two-level instance of
the target rules below; it is not a proof of Core or native code.

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

The first Core slice uses `borrow.begin p, mode` to create a scalar reference
and `borrow.end ref` to end it. A separate verifier tracks exact active loan
IDs through CFG edges. `borrow.reborrow ref, mode` records an explicit parent;
an exclusive parent is frozen until its child ends. A shared parent can only
grant a shared child. The verifier permits concurrent shared loans, rejects
an exclusive overlap, blocks direct mutation or exclusive reads of a loaned
root, rejects use after end or out-of-order parent end, and requires identical
active and frozen loan states at joins.
It requires loans to end before return, request or reply suspension, spawn,
or scope join. A loan may cross an ordinary call, `yield`, `channel.send`, or
`channel.receive`: frame analysis retains both the reference value and its
root place, and the native poll frame gives the root a stable address.
Interpreter, JIT, and standalone tests exercise direct loans, nested loans
spanning yield, and loans spanning channel send and receive. The receive test
runs both delivered and closed outcomes. Source `with r = &x { ... }` and
`with r = &mut x { ... }` emit these operations for scalar `val` or `var`
owners. A nested `with` may reborrow a lexical alias. The lowerer ends loans
in reverse order on normal and `?` exits. Source examples cover nested loans
across yield, an exclusive loan across channel receive, and an alias forwarded
to a suspendable helper. At a call, Core compares direct and lexical argument
roots, rejects an exclusive overlap or a frozen parent, and allows compatible
shared borrows. A deadlocked helper under a live loan has an interpreter and
native unwind test. Generic calls under a live lexical loan remain restricted.

Next, integrate the Core parent relation with incoming borrowed parameters so
they can be reborrowed as lexical aliases. Ordinary ownership and
capability-flow verification remain separate passes over the same explicit
Core edges. The native frame plan must extend the stable-root check to remaining
suspension points, and cancellation must release loans before their slots. These checks should
remain linear in Core size apart from the existing CFG fixed point, with no
optimization search during compilation.

Before adding source-level nominal or field borrows, implement all remaining
suspension and failure unwind paths for live loans, with forged-IR
rejection and interpreter/native differential cases. Product fields are
currently stored in separate native slots, so nominal borrowing also needs
a stable aggregate address or an explicit multi-address view. The Maude
model is backpressure on that transition design, not a substitute for
executable checks.
