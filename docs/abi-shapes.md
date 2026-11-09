# ABI shapes in the executable compiler

Status: implementation note, not a stable external ABI.

`crates/layout` now owns a backend independent `Shape` made of byte, word, and
address components. Cranelift maps those components to its scalar types;
the retained frame layout counts them as words. Distinct nominal types can
have equal transport shapes. This establishes a reusable shape key for
generic compilation, but shape equality alone grants no right to inspect,
copy, or close a value. `crates/abi` owns generic instance planning and
reexports these shapes for existing callers. Generic bodies and dictionaries
are not yet emitted.
`phoebe shapes <file|project>` prints each checked function's parameter and
result transport parts. It omits hidden context, suspension, dictionary
layout, and the target return convention, so its output is not a machine-code
sharing key. `docs/generic-core.md` specifies the full compatibility rule.

Core IR assigns one nominal type to each value. Cranelift lowering expands a
value into an ordered sequence of scalar components at function, call, return,
and block boundaries. The current shapes are:

| Phoebe type | Components |
| --- | --- |
| `I64` | one I64 |
| `Bool` | one I8 |
| `Unit` | no components |
| Nullary sum | one I64 variant tag |
| Sum with payload variants | I64 tag and enough I64 words for the largest flattened payload |
| Product of finite fields | concatenated field shapes, in declaration order |
| `&I64`, `&mut I64`, `&Bool`, `&mut Bool` | one native address |
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
its program counter, at least one word for its return and runtime receive
scratch area, then only its live
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
A checked single-block product constructor built from literals and moves can
write each field directly into that wide-result caller buffer. Selection
requires a direct return of the complete destination and excludes calls and
other effects after partial initialization. The direct Cranelift path then
omits the intermediate product stack loads and stores; retained tasks and
more complex constructors still use local destinations.
The shape table also records the part offset and kind of each capability leaf
in a product. A `Request<I64>` contributes its reply right at its second
part. Reverse traversal of this table gives structural close order without
a runtime scan. Spawn descriptors use the same leaf offsets to transfer all
nested rights to the child; source tests cover direct and retained parents.
For a capability-bearing sum, a flat leaf list cannot identify which payload
exists. The ABI crate therefore builds a `ClosePlan`: products sequence their
field plans in reverse order, and sums select one payload plan by tag. The
interpreter closes the same recursive value structure; direct and retained
Cranelift functions emit the plan. `Option<Sender<I64>>`,
`Result<Sender<I64>, ResourceError>`, and products containing those sums are
executable across interpreter, JIT, and standalone builds. Spawn transfer
uses the same structural plan to select the active payload's rights. Direct
and retained native parents build a descriptor array sized for the largest
possible variant, then pass the actual descriptor count to the runtime.
Arguments without tags keep constant descriptor offsets and counts. The
runtime validates every right before committing the child and supports
transfer lists up to the child frame's word count.

A sum with a payload reserves the maximum number of flattened payload words
among its variants. Every payload component is represented as an I64 word;
`Bool` components are zero-extended on construction and narrowed at the
matching arm. Unused words in a variant are zero-filled. The Core IR `match`
routes the active payload into the chosen arm's typed block parameter. A sum
with no payload variants keeps its single-word shape. This supports concrete
`Result<T, E>` and `Option<T>` applications whose arguments are primitive values,
existing nominal products or sums, or other finite `Result`/`Option`
applications. The verifier requires nested payload types to precede their
containing sum in the data table, so shape computation cannot recurse
indefinitely.

User-declared sums and products may contain earlier nominal types and finite
concrete `Result` and `Option` applications. Their shapes remain finite because
the verifier rejects a reference to the current or a later data declaration. A
closed nominal field projection preserves its owner, as does a primitive
field read. A closed sibling field may be read from a product with rights;
moving a right out still requires whole-product `unpack()`. Closed nominal
uses may be copied by the compiler and appear in the cost ledger. Explicit
`copy(x)` remains available. Sum payloads are accessed through `when` bindings.

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
