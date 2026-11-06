# Executable Core IR: current subset

Status: implementation note for the compiler under development. The current
IR is not yet Phoebe's complete checked IR or a proven semantics.

A function contains typed SSA values, numbered basic blocks, and numbered
mutable places. Each block has zero or more typed parameters, an instruction
sequence, and exactly one `return`, `jump`, or `branch` terminator. A `jump`
supplies one typed value per destination parameter. An `if` expression lowers
to a conditional branch and a join block whose parameter carries the chosen
arm's result. A `while` statement lowers to a loop head, body, exit, and
backedge.

`val` binds an SSA value. `var` creates a task-local scalar place. The current
place operations are `place.init`, `place.load`, and `place.store`. An
initializer dominates every use, including on a loop backedge. A place can
hold `I64` or `Bool`; no source reference can yet point to one. The interpreter
stores a value per place, while Cranelift lowers it to a typed stack slot.
This separation keeps the future reference and region rules attached to
*places*, rather than pretending a mutable object is an SSA value.

The structural verifier checks unique SSA definitions, operand dominance,
block reachability, parameter and result types, branch targets, call arity,
and one dominating initializer per place. Native lowering runs it before
code generation. This is an engineering check, not a proof that lowering is
correct. Differential tests compare interpreter and JIT results on supported
programs. The Maude models cover different bounded abstract machines; they
do not model this executable IR.

Next IR steps are explicit borrow loans rooted in places, region begin/end,
ownership moves, cleanup edges, and cost events. The verifier will need to
check those without turning ordinary compilation into proof search.
