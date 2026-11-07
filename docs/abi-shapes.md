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
| Sum with payload variants | I64 tag and enough I64 words for the largest flattened payload |
| Product of finite fields | concatenated field shapes, in declaration order |
| `&I64`, `&mut I64` | one native address |

For a product, `product` gathers existing SSA components and `field` selects
the complete component range of one field. Neither operation creates a native
heap object or a Core IR place. A
`var` product gets one task-local stack slot per component. An owning call
passes the components directly, and a return yields them directly. The
interpreter uses an explicit product value; its internal allocations are not
part of the native runtime.

A sum with a payload reserves the maximum number of flattened payload words
among its variants. Every payload component is represented as an I64 word;
`Bool` components are zero-extended on construction and narrowed at the
matching arm. Unused words in a variant are zero-filled. The Core IR `match`
routes the active payload into the chosen arm's typed block parameter. A sum
with no payload variants keeps its single-word shape. This supports concrete
`Result<T, E>` applications whose arguments are primitive values, existing
nominal products or sums, or other finite `Result` applications. The verifier
requires nested payload types to precede their containing sum in the data
table, so shape computation cannot recurse indefinitely.

User-declared sums and products may contain earlier nominal types. Their
shapes remain finite because the verifier rejects a reference to the current
or a later data declaration. A nominal product field projection consumes its
owner; a primitive field read leaves the owner live. A nested nominal value
can be duplicated only with explicit `copy(x)`, which appears in the cost
ledger. User-declared fields of a concrete `Result` application remain
unsupported.

This flattening is a compiler-internal calling convention. The external
entrypoint remains a scalar I64 function, and the standalone C wrapper only
calls a zero-argument `main` returning I64. A later shape policy must decide
when a larger aggregate switches to indirect passing or explicit copying;
the current compiler does not enforce that cost threshold. Generic
shape sharing and general capability ownership remain unimplemented. Affine
nominal moves and explicit fixed-size copies are checked in Core IR.

The verifier checks the nominal data ID, field count, field types, and field
projection index before native lowering. A future proof obligation is that
flattening preserves typed Core IR evaluation, including joins and calls, and
that no aggregate temporary is introduced unless a source-level operation
requires one. Differential tests currently provide bounded evidence for this
claim; they are not a proof.
