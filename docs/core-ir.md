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
hold `I64` or `Bool`. A synchronous call can borrow an `I64` place through
`&I64` or `&mut I64`; the callee uses `ref.load` and, for an exclusive loan,
`ref.store`. The interpreter stores a value per place and resolves the call
borrow to the caller's slot. Cranelift lowers places to typed stack slots and
passes their addresses. This separation keeps reference and region rules
attached to *places*, rather than pretending a mutable object is an SSA value.

The structural verifier checks unique SSA definitions, operand dominance,
block reachability, parameter, call, and return types, branch targets, call modes,
one dominating initializer per place, and the restricted borrow rules in
`call-borrows.md`. Native lowering runs it before
code generation. This is an engineering check, not a proof that lowering is
correct. Differential tests compare interpreter and JIT results on supported
programs. The Maude models cover different bounded abstract machines; they
do not model this executable IR.

Next IR steps are general loan lifetimes and region begin/end, ownership
moves, cleanup edges, and cost events. The verifier will need to
check those without turning ordinary compilation into proof search.
