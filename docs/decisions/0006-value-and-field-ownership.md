# Decision 0006: Value semantics with field-sensitive resource ownership

Status: accepted, 2026-11-08. This supersedes Decision 0003's rule that every
nominal `T` argument consumes its caller's ordinary value and that independent
ordinary values require source-level `copy(x)`. Implementation is incremental;
the current executable subset is recorded separately in `version-0.0.1.md`.

## Source contract

Closed data has value semantics. Using an ordinary value does not make another
source binding invalid. A function taking `T` receives the value described by
its signature; whether the compiler borrows its storage, moves its sole owner,
retains immutable storage, or copies on write does not change the caller's
rights. A private callee edit cannot change this source contract. `var` may
replace its value; the compiler may update unique storage in place, and must
preserve any other live value when storage is shared. `copy(x)` may remain as
an explicit request for independent storage but is not required for ordinary
reuse. Compiler-generated retain, release, allocation, and byte-copy sites
must have source reasons and be visible in the cost ledger.

Communication capabilities are resources, not ordinary closed data. Each
sender, receiver, and reply right has one owner at a time. A call, spawn,
message, field extraction, or return transfers a right; `fork()` explicitly
creates a distinct right and can fail. Repeated operations by the current
owner do not duplicate the right. `with` and structural cleanup close any
right still owned on an exit. An ordinary closed field of a resource-bearing
record may be retained independently while a sibling right transfers.
Retaining the entire resource-bearing record cannot implicitly duplicate its
rights. This refines Decision 0004 without changing its exact cleanup rule.

References and destinations are compiler mechanisms for the common path.
The compiler infers short loans from ordinary value uses; it tracks loans by
root and field path and permits disjoint fields to be used independently.
A loan of a parent conflicts with mutation or transfer of a child. First-class
references stored in data, cross-task references, and source-level destination
types are outside this decision. Result construction may write directly to a
caller-owned destination, which is unpublished until initialized; failure
cleans exactly the initialized owned fields. The path checker may use regular
languages internally, but no regular-expression syntax is exposed.

## Required invariants

1. An ordinary value observed after a call has the same logical contents it
   had before the call unless the source explicitly reassigns that binding.
2. Every active capability has exactly one owner. Field moves leave a tracked
   empty slot; they cannot silently copy, close, or resurrect the right.
3. Every read targets an initialized field. Every parent/child or overlapping
   exclusive loan is rejected. Disjoint field loans may coexist.
4. A result is published only when its active variant and all required fields
   are initialized. Error, cancellation, and deadlock cleanup visit exactly
   the initialized, still-owned fields once.
5. Inference and storage selection preserve public value semantics and
   expose all potentially material runtime work in the checked cost ledger.

The executable field model and its current limitations are in
`../ownership-prototype.md`. This decision commits to source semantics, not
to the model's Rust `Rc`, bounded regex search, or boxed destination as
production representations.
