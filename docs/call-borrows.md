# Call-site scalar borrows

Status: implemented subset, 2026-11-06. This is a step toward Phoebe's
general reference and region design, not a proof of it.

A function may declare `&I64` or `&mut I64` parameters. A caller writes `&x`
or `&mut x` as a direct call argument. The loan begins at the call after value
arguments have been evaluated, and ends when the callee returns. `&x` can read;
`&mut x` can read and assign the caller's scalar place. A mutable local has a
stable place. A shared borrow of a scalar `val` materializes a temporary
read-only place; a scalar `val` cannot be mutably borrowed.

The source checker rejects a call with overlapping arguments if any loan to
that place is exclusive. It permits repeated shared loans. A borrowed
parameter may be forwarded to another call with a compatible mode. Borrowed
parameters cannot be called as entry points from the CLI because the CLI has
no Phoebe owner supplying a live place.

Core IR represents a call argument as a value, a borrowed place, or a
forwarded reference parameter. Callee reference values are confined to
`ref.load`, `ref.store` when exclusive, or a forwarded call argument. They
cannot be returned or stored in another place. The verifier checks call
signature modes, alias conflicts within a call, place initialization and
dominance, reference operation types, and reference provenance restrictions.
The interpreter resolves references to caller-owned slots. Direct Cranelift
calls receive addresses of caller stack slots. When a callee can suspend, the
frame analysis retains the borrowed caller place in the stable parent frame;
the child frame stores its address until the call returns. Borrowed parameters
and forwarded references can be read or written after a channel receive.
The generated poll entry rejects borrowed external parameters because no
Phoebe caller owns a place for them. Interpreter, JIT, and standalone tests
exercise mutable forwarding and a shared borrow of a `val` temporary across
the receive. This is tested behavior, not a general proof of region safety.

This subset has no first-class reference variables, field or element borrows,
region inference, or cross-task references. It does not
establish safety for the future general borrow system. The next step is to
add explicit region roots and loan lifetimes to checked IR before extending
references beyond the current call scope.
