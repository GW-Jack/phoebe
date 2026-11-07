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
| Bare `SenderI64`, `ReceiverI64`, `ResourceError` | one I64 handle or error code |

Functions that perform channel runtime operations, and their transitive callers,
receive a hidden pointer to the current scoped runtime context. Unrelated
functions keep their existing signatures. Calls forward the pointer only when
the callee requires it. `channel.new.i64` calls a C ABI shim with the context, capacity,
and two output slots. It branches on the returned status and loads the slots
only on success. `close.handle` calls a separate shim. Standalone builds link
the same shim and scheduler from `libphoebe_runtime.a`, so this Core slice has one
runtime implementation across interpreter, JIT, and object code.

The runtime archive now exports versioned scalar send and receive shims.
Their statuses distinguish successful transfer, a parked operation, and
closed/end. A pending send leaves its message and sender with the caller; a
pending receive leaves its receiver with the caller and does not write the
output value. Generated code must retain those operands in a task frame and
return to the scheduler before it can use the shims. The retained Cranelift
path emits these calls for the supported scalar channel subset.
The retained-frame plan now assigns one `I64` word per flattened component
using these same shapes. A suspendable function's frame reserves word 0 for
its program counter, enough words for its return shape, then only its live
value and place components. `docs/native-frames.md` records the poll ABI and
the remaining code-generation work.

For a product, `product` gathers existing SSA components and `field` selects
the complete component range of one field. Neither operation creates a native
heap object or a Core IR place. A `var` product gets one task-local slot per
component, in the frame when it crosses a suspension. An owning call passes
the components directly. Direct returns use registers for up to two
components on x86-64 or eight on AArch64; wider returns write I64 words to
an explicit caller-owned stack buffer. Bool components are widened on store
and narrowed on load. This follows the available return registers without
Cranelift's deprecated implicit multi-return buffer. The
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

User-declared sums and products may contain earlier nominal types and finite
concrete `Result` applications. Their shapes remain finite because the verifier
rejects a reference to the current or a later data declaration. A nominal
product field projection consumes its
owner; a primitive field read leaves the owner live. A nested nominal value
can be duplicated only with explicit `copy(x)`, which appears in the cost
ledger. User-declared fields of a concrete `Result` application remain
unsupported.

This flattening is a compiler-internal calling convention. The external
entrypoint remains a scalar I64 function, and the standalone C wrapper only
calls a zero-argument `main` returning I64. The target-dependent return
threshold is an ABI compatibility rule, not a measured optimal cost threshold.
A later shape policy must decide when larger arguments switch to indirect
passing or explicit copying. Generic
shape sharing and general capability ownership remain unimplemented. Affine
nominal moves and explicit fixed-size copies are checked in Core IR.

The verifier checks the nominal data ID, field count, field types, and field
projection index before native lowering. A future proof obligation is that
flattening preserves typed Core IR evaluation, including joins and calls, and
that the wide-result buffer is introduced only when the ABI requires it.
Differential tests currently provide bounded evidence for this
claim; they are not a proof.
