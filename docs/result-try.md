# Executable `?` rule

Status: concrete built-in `Result<T, E>` applications.

The current source subset accepts concrete `Result<T, E>` applications with
primitive, existing nominal, or finite nested `Option`/`Result` arguments. `ResourceError` is
also a closed error payload. Each application
is a distinct checked type with `Ok` and `Err` variants carrying the
corresponding payload. Constructors and patterns spell the application
explicitly, for example `Result<I64, Bool>.Err(true)`. A user declaration
named `Result` is reserved. `?` requires the enclosing function to return a
concrete `Result<U, E>` with the same error type `E`; the success types may
differ. Generic functions and error conversion are
still unimplemented. User data may store finite concrete `Result`
applications when their nominal argument types were declared earlier.

For `val x = expression?`, lowering evaluates `expression` once, then emits a
Core IR `match`. The `Ok` target receives its payload as a typed block
parameter and continues with `x`. When the input and return types are identical,
the `Err` target receives the whole original `Result` through a block parameter
and returns it without reconstructing the value. When only the success types
differ, the `Err` target receives the error payload and constructs one
`Result<U, E>.Err` for the return edge. No error conversion or runtime helper
is inserted. The verifier checks whether each match edge
passes its payload, its whole subject, or nothing, and checks the target
parameter type accordingly.

The first source channel binding is a special pair pattern over a fallible
creation edge: `val (sender, receiver) = channel<I64>(capacity = n)?`.
On success, the two fresh endpoint block parameters become the named owners.
On failure, the `ResourceError` block parameter is wrapped in the enclosing
`Result<_, ResourceError>.Err` and returned. No endpoint exists on that edge.
This is not yet a general expression-valued channel pair or a general
conversion from one error type to another.

This is a normal return edge. After lowering, the exact capability-flow
checker finds endpoints still owned at each source `?` error return, and
lowering inserts a close for each of those rights on that edge. The ordinary
Core verifier then checks the completed graph. This covers bare scalar
channel endpoints in the current source subset; nested capability-bearing
data and general lexical cleanup are not implemented. The existing Maude
models do not prove this compiler lowering.
The useful proof obligation is: for any checked expression yielding
`Result<T, E>`, `?` evaluates it once, propagates the same error payload on failure,
and yields the `Ok` payload on success while preserving the same ownership
state on every outgoing edge.
