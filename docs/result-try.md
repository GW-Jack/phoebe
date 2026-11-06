# Executable `?` rule

Status: concrete built-in `Result<T, E>` applications.

The current source subset accepts concrete `Result<T, E>` applications with
primitive, existing nominal, or nested `Result` arguments. Each application
is a distinct checked type with `Ok` and `Err` variants carrying the
corresponding payload. Constructors and patterns spell the application
explicitly, for example `Result<I64, Bool>.Err(true)`. A user declaration
named `Result` is reserved. `?` requires the enclosing function to return
that same concrete application. Generic functions, user-declared nested
payloads, and error conversion are still unimplemented.

For `val x = expression?`, lowering evaluates `expression` once, then emits a
Core IR `match`. The `Ok` target receives its payload as a typed block
parameter and continues with `x`. The `Err` target returns the original
`Result` value. It does not construct another `Err`, copy a payload into a
temporary, call a runtime helper, or perform a hidden conversion. The match
target for `Err` intentionally discards its payload parameter. The verifier
allows a payload-bearing match target to take zero or one payload parameter;
if present, its type must match the variant declaration.

This is a normal return edge. Once lexical capability owners exist, lowering
must insert their structural closes on the error edge before returning. The
current executable subset has no such owners, so no cleanup operation is
needed yet. The existing Maude models do not prove this compiler lowering.
The useful proof obligation is: for any checked expression yielding
`Result<T, E>`, `?` evaluates it once, returns the exact `Err` value on failure,
and yields the `Ok` payload on success while preserving the same ownership
state on every outgoing edge.
