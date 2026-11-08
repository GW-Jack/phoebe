# Call-site scalar borrows

Status: implemented subset, 2026-11-06. This is a step toward Phoebe's
general reference and region design, not a proof of it.

A function may declare `&I64`, `&mut I64`, `&Bool`, or `&mut Bool` parameters.
A caller writes `&x` or `&mut x` as a direct call argument. The loan begins
at the call after value arguments have been evaluated, and ends when the
callee returns. `&x` can read; `&mut x` can read and assign the caller's
scalar place. A mutable local has a stable place. A shared borrow of a scalar
`val` materializes a temporary read-only place; a scalar `val` cannot be
mutably borrowed. The native ABI passes one address for either scalar, with
the referent's own load and store width.

The source checker rejects a call with overlapping arguments if any loan to
that place is exclusive. It permits repeated shared loans. A borrowed
parameter may be forwarded to another call with a compatible mode. Borrowed
parameters cannot be called as entry points from the CLI because the CLI has
no Phoebe owner supplying a live place.

Core IR represents a call argument as a value, a borrowed place, or a
forwarded reference parameter. A call with any borrowed argument has a
`LoanRegion` record in its caller. It identifies the call instruction and
the roots in argument order; the region begins immediately before that call
and ends when it returns or unwinds. The Core verifier rejects absent,
duplicate, or mismatched records. Callee reference values are confined to
`ref.load`, `ref.store` when exclusive, or a forwarded call argument. They
cannot be returned or stored in another place. The verifier checks call
signature modes, alias conflicts within a call, place initialization and
dominance, reference operation types, and reference provenance restrictions.
It also rejects reference types in nominal product fields and sum payloads;
otherwise a forged Core aggregate could hide a loan beyond its call region.
The interpreter resolves references to caller-owned slots. Direct Cranelift
calls receive addresses of caller stack slots. When a callee can suspend, the
frame analysis retains the borrowed caller place in the stable parent frame
and checks it against the loan-region roots;
the child frame stores its address until the call returns. Borrowed parameters
and forwarded references can be read or written after a channel receive.
The generated poll entry rejects borrowed external parameters because no
Phoebe caller owns a place for them. Interpreter, JIT, and standalone tests
exercise mutable forwarding and a shared borrow of a `val` temporary across
the receive. This is tested behavior, not a general proof of region safety.

Call-site borrowing remains one subset. Scoped scalar reference aliases now
use `with r = &x { ... }` or `with r = &mut x { ... }` and explicit Core begin/end
operations. Ordinary calls can now run inside a live lexical loan. A lexical
alias may be forwarded to a callee, including one that suspends; Core checks
the direct and lexical argument roots and the frame retains local roots.
An incoming borrowed scalar parameter can also be reborrowed in a shorter
lexical `with` region. Core treats that parameter as an external storage root,
freezes an exclusive parent while its child is live, and checks aliases when
the child is forwarded to another call. The caller retains the owned place
across suspension; the callee retains the reference value. Generic calls
under a live loan are supported for the current owned-argument,
non-suspending generic subset.
Field or element borrows, region inference, and cross-task references remain
unsupported. See `docs/lexical-regions.md` for that boundary.
