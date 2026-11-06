# ABI shapes in the executable compiler

Status: implementation note, not a stable external ABI.

Core IR assigns one nominal type to each value. Cranelift lowering expands a
value into an ordered sequence of scalar components at function, call, return,
and block boundaries. The current shapes are:

| Phoebe type | Components |
| --- | --- |
| `I64` | one I64 |
| `Bool` | one I8 |
| Nullary sum | one I64 variant tag |
| Product of primitive fields | one component per field, in declaration order |
| `&I64`, `&mut I64` | one native address |

For a product, `product` gathers existing SSA components and `field` selects
one. Neither operation creates a native heap object or a Core IR place. A
`var` product gets one task-local stack slot per component. An owning call
passes the components directly, and a return yields them directly. The
interpreter uses an explicit product value; its internal allocations are not
part of the native runtime.

This flattening is a compiler-internal calling convention. The external
entrypoint remains a scalar I64 function, and the standalone C wrapper only
calls a zero-argument `main` returning I64. A later shape policy must decide
when a larger aggregate switches to indirect passing or explicit copying;
the current compiler does not enforce that cost threshold. Fields are limited
to `I64` and `Bool`. Nested products, payload-bearing sums, generic shape
sharing, and ownership moves remain unimplemented.

The verifier checks the nominal data ID, field count, field types, and field
projection index before native lowering. A future proof obligation is that
flattening preserves typed Core IR evaluation, including joins and calls, and
that no aggregate temporary is introduced unless a source-level operation
requires one. Differential tests currently provide bounded evidence for this
claim; they are not a proof.
