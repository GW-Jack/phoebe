# Executable Core IR: current subset

Status: implementation note for the compiler under development. The current
IR is not yet Phoebe's complete checked IR or a proven semantics.

A function contains typed SSA values, numbered basic blocks, and numbered
mutable places. Each block has zero or more typed parameters, an instruction
sequence, and exactly one `return`, `jump`, `branch`, `match`, or
`channel.new.i64` terminator. A `jump`
supplies one typed value per destination parameter. An `if` expression lowers
to a conditional branch and a join block whose parameter carries the chosen
arm's result. A `while` statement lowers to a loop head, body, exit, and
backedge.

Nominal sum variants have a distinct `Data(id)` type and an explicit `variant`
instruction. Source `when` checks every variant exactly once, then lowers to
one `match` terminator with a target per variant. Each edge explicitly binds
the active payload, the whole subject, or nothing. The verifier checks the tag
bounds and the edge binding against its target's parameter type and count.
Payload access therefore
does not need a separate unchecked field operation or path-sensitive proof
search. Native lowering expands the match into a tag dispatch; the interpreter
selects the matching arm directly. Finite products have `product` and
`field` operations. Their fields remain separate SSA components through native
calls, returns, and joins; `docs/abi-shapes.md` specifies the current shape.

The first `?` lowering uses this same `match` terminator. Its `Err` target
receives and returns the original sum value through a block parameter; its
`Ok` target binds the payload. There is no new `Err` construction. See
`docs/result-try.md` for the executable rule and missing cleanup obligation.

`val` binds an SSA value. `var` creates a task-local place. The current
place operations are `place.init`, `place.load`, `place.take`, `place.field`,
and `place.store`. An
initializer dominates every use, including on a loop backedge. A place can
hold `I64`, `Bool`, sums, or finite products. A synchronous call can
borrow an `I64` place through
`&I64` or `&mut I64`; the callee uses `ref.load` and, for an exclusive loan,
`ref.store`. The interpreter stores a value per place and resolves the call
borrow to the caller's slot. Cranelift lowers places to typed stack slots and
passes their addresses. This separation keeps reference and region rules
attached to *places*, rather than pretending a mutable object is an SSA value.

Nominal values are currently affine. Reading a nominal `val` for an owning
use emits `move`; reading a nominal `var` emits `place.take`. A primitive
`copy(x)` emits `copy.value` for a nominal owner and preserves the source.
That IR operation is also an on-demand static cost event. A primitive
product field read uses `field` on a live SSA owner or `place.field` on a live
place, leaving the owner available. A nominal product field uses an owning
`field` projection; a `var` first emits `place.take`. The other closed fields
are discarded. The ownership verifier propagates live
value and place states through the CFG, consumes them at owning calls,
constructions, jumps, returns, and matches, and intersects them at joins.
Using an owner after a move receives a source-located diagnostic. The native
`move` reuses the same scalar components; `place.field` loads only the selected
primitive component. The interpreter's boxed values remain a reference implementation,
not a native allocation model. `affine-ownership.md` gives the abstract
transition rules and their evidence boundary.

The structural verifier checks unique SSA definitions, operand dominance,
block reachability, parameter, call, and return types, branch targets, call modes,
one dominating initializer per place, and the restricted borrow rules in
`call-borrows.md`. Native lowering runs it before
code generation. This is an engineering check, not a proof that lowering is
correct. Differential tests compare interpreter and JIT results on supported
programs. The Maude models cover different bounded abstract machines; they
do not model this executable IR.

The current nominal-value check is affine: unused closed values may be dropped.
Replacing an owned `var` currently discards the old closed value, and `match`
may discard a payload. The new endpoint-only capability check has exact
disposition at return, but nested capabilities still need structural cleanup
or explicit transfer on those exits.
General loan lifetimes, region begin/end, cleanup edges,
and cost events remain next steps. The verifier will need to check those
without turning ordinary compilation into proof search.
`capability-ir.md` gives the intended edge accounting and suspension rules
for the next implementation slice.

`channel.new.i64` is currently a checked Core-only edge. It takes an `I64`
capacity and has a success successor with `SenderI64` and `ReceiverI64` block
parameters, plus a failure successor with a `ResourceError` parameter.
`close.handle` consumes either endpoint. Core derives a capability-flow graph
from these operations: only the success edge creates the two rights, jumps
move rights to destination parameters, and normal return requires both rights
to be closed. This is exercised with hand-built Core programs. The interpreter
runs creation and close through a one-task scheduler with explicit scope limits,
including resource-failure edges. `interpret_with_limits` can vary those
limits; plain `interpret` permits one task and 64 channel creations. Cranelift
JIT execution uses the same scheduler through a hidden context parameter and
agrees on the tested success and failure paths. Source lowering and standalone
runtime linking remain open.
