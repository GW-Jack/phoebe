# Lexical borrow regions

Status: executable scalar source and Core slice plus remaining design
obligations. Source supports call-scoped loans and scoped `with` bindings for
I64 and Bool places. Checked Core supports explicit `borrow.begin`,
`borrow.reborrow`, and `borrow.end` for task-local scalar places and reborrows
of incoming borrowed scalar parameters.
The finite `formal/lexical_borrows.maude` model checks a two-level instance of
the target rules below; it is not a proof of Core or native code.

## Ownership and authority

Every loan has a stable storage root, a mode (shared or exclusive),
and a lexical region. Its region must end before the storage root is released:
`lifetime(loan) ⊆ lifetime(root)`. A move, drop, or replacement of that root
requires no live loan. A shared loan permits reads and further shared loans.
An exclusive loan permits reads and writes. Reborrowing an exclusive loan
freezes the parent authority until the child loan ends; borrowing through a
shared loan cannot create exclusive authority. A reference never owns its
root and cannot cross a task or channel boundary. A callee may reborrow an
incoming reference; its caller retains ownership of the root.

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
It requires loans to end before return. A loan may cross an ordinary call,
`yield`, channel, request, or reply traffic, spawn, and scope join. Frame
analysis retains the reference value and any task-local root place at each
suspension point; the native poll frame gives that root a stable address.
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
native unwind test. Reply receive tests cover delivered and closed outcomes;
request tests cover a pending receive and a full send queue. A scoped spawn
test covers a pending join and task-limit failure with the loan still live.
An owned-argument generic call can run under a live loan;
the current generic body subset cannot suspend or accept borrowed arguments.
An incoming borrowed scalar parameter can parent a shorter
lexical loan. The verifier gives it an external root identity, freezes an
exclusive parent while the child is active, and checks call aliases against
that identity. Across suspension, the caller retains the place and the callee
retains its reference value.

Ordinary ownership and capability-flow verification remain separate passes over the same explicit
Core edges. The scheduler now drops descendant task frames before ancestor
frames on failure and deadlock. A source regression cancels a pending borrowed
call after a child faults. Broader failure unwind coverage remains open. These checks should
remain linear in Core size apart from the existing CFG fixed point, with no
optimization search during compilation.

Before adding source-level nominal or field borrows, finish failure unwind
coverage for live loans, with forged-IR
rejection and interpreter/native differential cases. Product fields are
currently stored in separate native slots, so nominal borrowing also needs
a stable aggregate address or an explicit multi-address view. The Maude
model is backpressure on that transition design, not a substitute for
executable checks.
