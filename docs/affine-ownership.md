# Affine ownership in the executable Core IR

Status: implemented for nominal values in the current closed-value subset.
This is a step toward capability ownership, not a memory-safety proof for
tasks, channels, or references.

`I64` and `Bool` are copyable. Every nominal `Data(id)` value is currently
affine, even if its fields are small. The source can read primitive product
fields without consuming the owner. An owning use transfers the whole value.
A nested nominal field projection consumes the product owner and returns the
field; other closed fields are discarded. Reading a nominal field from a
mutable place first takes the whole product. A second access requires an
explicit `copy(x)` before the first owning projection.
There is no implicit copy of a nominal value. `copy(x)` explicitly creates a
second owned closed value and leaves `x` live; a scalar `copy(x)` is an identity
use. Every nominal copy appears as `copy.value` in Core IR and the cost ledger.

For a function, let `L_v` be the set of live nominal SSA values and `L_p` the
set of live nominal places. The verifier applies these transitions in program
order:

| IR action | Required state | Resulting state |
| --- | --- | --- |
| Nominal definition | — | Add its value to `L_v` |
| `move v` | `v ∈ L_v` | Remove `v`; add the result |
| `place.take p` | `p ∈ L_p` | Remove `p`; add the result |
| `copy.value v` or `copy.value p` | Owner live | Keep the owner; add the independent result |
| Primitive `field v, i` or `place.field p, i` | Owner live | State unchanged |
| Nominal `field v, i` | `v ∈ L_v` | Remove `v`; add the projected field |
| Owning call, construction, store, jump, return | Each nominal operand live | Remove transferred operands; store makes its destination place live |
| `match v` | `v ∈ L_v` | Remove `v`; each edge introduces its explicit payload or whole-subject block parameter |

At a CFG join, an owner is definitely live only if it is live on every
incoming edge: `L_join = ⋂ L_predecessor`. The forward worklist shrinks these
sets to a fixed point, including loop backedges. A later use of an absent
owner is rejected. Source lowering maps an owning name use to `move` or
`place.take`, so the diagnostic points at that use.

This is an affine rule: an unused closed value may be dropped. Reassignment
currently drops an old closed value; a match arm can discard a payload. Once
a type contains a communication capability, those transitions must close or
transfer every live capability structurally. The current verifier does not
establish that property. It also does not prove the Rust implementation of
the dataflow algorithm; compiler tests include forged IR, branch and loop
paths, and interpreter/native agreement as bounded evidence.
