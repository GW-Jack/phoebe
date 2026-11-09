# Affine ownership in the executable Core IR

Status: implemented for closed nominal reuse and owning capability-bearing
uses in the current source subset. Closed nominal name and field reads preserve
their source and create an independently usable value. Capability-bearing
partial field moves are not yet implemented.
A separate exact capability-flow verifier checks communication
rights. Neither checker is a proof of the compiler, runtime, or generated code.

`I64`, `Bool`, and `Unit` are copyable. A closed nominal `Data(id)` has source
value semantics. A nominal value containing a communication right is affine;
an owning use transfers the whole value.
A nested closed nominal field projection preserves its product owner and
returns an independently usable field value. The same applies to a closed
nominal field read from a mutable place. The current representation may copy
its flattened components; `phoebe cost` marks this as an inferred field value.
Closed fields of capability-bearing products can also be read while the right
stays in its parent. Extracting a right still requires whole-product unpack.
An ordinary closed nominal name use emits `inferred.copy` or
`inferred.copy.place`; the last SSA use on a return path instead moves the
source. `copy(x)`
explicitly creates a second closed value and leaves `x` live; a scalar
`copy(x)` is an identity use. Inferred and explicit copies appear as
`copy.value` cost sites with distinct reasons. These are currently fixed-size
component copies, not shared storage or copy-on-write.

For a function, let `L_v` be the set of live nominal SSA values and `L_p` the
set of live nominal places. The verifier applies these transitions in program
order:

| IR action | Required state | Resulting state |
| --- | --- | --- |
| Nominal definition | — | Add its value to `L_v` |
| `move v` | `v ∈ L_v` | Remove `v`; add the result |
| `place.take p` | `p ∈ L_p` | Remove `p`; add the result |
| Capability `place.store p, v` | `v` live; `p` empty | Remove `v`; add `p` |
| Explicit or inferred copy from `v` or `p` | Owner live | Keep the owner; add the independent result |
| Primitive `field v, i` or `place.field p, i` | Owner live | State unchanged |
| Closed nominal `field v, i` | `v ∈ L_v` | Keep `v`; add an independent field value |
| Owning call, construction, store, jump, return | Each nominal operand live | Remove transferred operands; store makes its destination place live |
| `match v` | `v ∈ L_v` | Remove `v`; each edge introduces its explicit payload or whole-subject block parameter |

At a CFG join, an owner is definitely live only if it is live on every
incoming edge: `L_join = ⋂ L_predecessor`. The forward worklist shrinks these
sets to a fixed point, including loop backedges. A later use of an absent
owner is rejected. Source lowering maps capability-bearing name uses to
`move` or `place.take`, so the diagnostic points at that use. Closed values
may be copied at each source use while liveness and storage inference develop.

This is an affine rule: an unused closed value may be dropped. Reassignment
currently drops an old closed value; a match arm can discard a payload. Once
a type contains a communication capability, those transitions must close or
transfer every live capability structurally. The exact capability-flow graph
checks that property for the supported right types, including rights held in
`var` places and restored after send, receive, or fork. An owned take empties
the place; a second take and a store over a live right are rejected. This
verification does not prove the Rust implementation of the dataflow algorithm;
compiler tests include forged IR, branch and loop paths, and
interpreter/native agreement as bounded evidence.
