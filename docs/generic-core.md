# Parametric Core and shape-shared generic bodies

Status: implementation contract. Generic function definitions are not yet
accepted by the executable compiler. This document sets the representation
and verification obligations before adding syntax or native emission.

## The sharing rule

A generic definition has one source body under universally bound type
variables. The compiler checks that body once under those variables. At a
concrete call, it substitutes types in the signature, computes their ABI
transport shapes, and selects a native body by a canonical key. Two calls
may share machine code only if all of these match:

1. The normalized parametric body and its checked operation effects.
2. The ordered parameter and result transport shapes and parameter modes.
3. The target calling convention, including return-buffer and hidden runtime
   context decisions, and whether the body can suspend.
4. The dictionary *layout* required by the body. Dictionary contents may
   differ by concrete type and are passed at the call site.

Equal transport shapes alone are insufficient. `I64` and `Sender<I64>` each
occupy one word, but one may be copied and the other must be transferred or
closed. A body that knows only `T` must not copy, inspect fields, close a
right, or synthesize a value of `T` without a checked bound and an explicit
dictionary operation. Moving an opaque `T` is allowed. If the body needs no
type-specific operation, it passes no dictionary pointer. A generic identity
should therefore compile to the same scalar transport as a concrete
identity, with no implicit allocation or dynamic call.

## Checked representation

Keep the existing concrete Core for monomorphic functions. Add a separate
parametric Core over type terms `Concrete(c)` and `Var(index)`, where indices
refer to a canonical binder order. A `Var` is an opaque affine owner,
regardless of whether an eventual substitution is copyable. Its legal base
operations are parameter admission, move, block argument, owned call, and
return. No field offsets or concrete capability leaves are assumed in that
body. A bound such as `Copy`, `Close`, or a named trait adds a typed dictionary
operation; its cost event records the source bound and call site.

The generic verifier checks types, control flow, region lifetimes, and exact
ownership of each opaque owner. It rejects a normal return with an
undisposed potentially capable owner. It must also check each dictionary
operation's declared ownership effect. Concrete Core at a call site verifies
the substituted signature and dictionary witness. The two verifiers have
distinct jobs: checking a generic definition must not replay the whole body
for every instantiation.

For a substitution `σ`, let `expandσ` map each opaque owner to the concrete
capability leaves of its type. The principal soundness obligation is:

> If a parametric Core transition is accepted, then for every well-formed
> substitution and dictionary satisfying its bounds, its concrete transition
> preserves exact leaf ownership under `expandσ`.

Moves transfer the entire opaque bundle, including the empty bundle for a
closed type. A dictionary operation needs a separately checked effect
contract. This simulation is a property of the verifier and elaborator,
established offline; it is not a new per-call compile-time proof search.
Finite Maude models can catch missed cases, while a general mechanized proof
must cover arbitrary control-flow graphs and substitutions. Until that proof
exists, tests and models are bounded evidence only.

## Dictionaries and ABI

The first executable generic subset should support identity and forwarding
through owned calls, branches, and returns. These require no dictionary.
Later bounds add dictionary entries for operations that depend on `T`. A
`Close` entry must use the same structural close plan as concrete Core,
including the active tag of a sum; a trait entry carries its callable target
and an explicit ownership signature. Dictionary layout is versioned and
included in the body key. No dictionary is consulted for a plain move.

Source interfaces record generic binders, bounds, parameter modes, and type
terms. Their canonical encoding includes binder order but excludes local
parameter spelling and source spans. The generic body hash is separate from
the public interface hash: changing a private implementation can rebuild its
native body without forcing downstream type checking when the interface is
unchanged. A native cache key includes the normalized body, transport and
call shape, dictionary layout, target, and compiler version.

## First executable acceptance cases

- `identity<T>(x: T) -> T` works for `I64`, `Unit`, two distinct closed
  nominal products of the same shape, and a product containing a channel
  right. Equal shapes share one emitted body when their body operations and
  call conventions agree.
- A generic owner is moved, never silently copied. A call returning a right
  still requires the caller to close or transfer it.
- A branch or `?` exit accounts for every opaque owner; a failed later
  argument closes an earlier owned temporary.
- Interpreter, JIT, and standalone results agree; printed parametric Core
  shows type binders and dictionary calls, and the cost ledger identifies
  each dynamic operation.
- An invalid generic body is diagnosed at its definition. No number of call
  sites makes verification time proportional to the full body size.

The current `phoebe shapes` command exposes concrete transport signatures.
It is a diagnostic for this work, not yet a generic sharing key: hidden
context, suspension, body effects, dictionary layout, and target ABI still
need to be added before native bodies can be shared.
