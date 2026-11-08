# Lexical task scopes: Core contract

Status: design for the next compiler integration. The Rust scheduler and its
native entry points execute this boundary; Phoebe source and Core IR do not
yet represent it.

`scope { body }` creates a child lifetime inside the current task. Every task
spawned while that body runs, including descendants spawned by those tasks,
belongs to the scope. Leaving the body waits for that set to finish. Tasks
started before the block remain in the enclosing scope and do not delay this
join. A child fault cancels the outer runtime scope, including siblings;
ordinary completion closes only the lexical scope. The first source form is a
statement. A later expression form may carry a result through the join.

## Checked IR representation

The intended Core operations are an effectful `scope.enter r` instruction and
a `scope.join r -> resume` terminator. `r` is a compiler-assigned region ID in
the function's scope-region table, not a transferable runtime value. A join
may park and retry the same terminator. The region table records its source
span and enter site; each normal or early exit has a join site. Generated code
calls `phoebe_rt_v1_scope_enter` and `phoebe_rt_v1_scope_join`. The scheduler
keeps the current scope in the task record, so the region ID needs no native
frame word or channel message representation.

The Core verifier propagates a stack of region IDs through every block. An
enter pushes its ID; a join must match and pop the top ID on its resume edge.
All incoming edges to a block must agree on the complete stack. A return
requires an empty stack, and a region cannot be entered twice while already
open. This checks LIFO closure through branches, loops, calls, and `?`
without running a proof search in ordinary compilation. Source lowering must
route early returns through joins in reverse lexical order, after cleanup of
rights whose own lexical scopes have ended. A failed scope entry faults the
task before publishing a region; the outer scheduler cancels the remaining
tasks. A future typed resource outcome can replace that fault without
changing the join invariant.

Let `home(t)` be the runtime scope in which task `t` was spawned, and let
`ancestor(s, r)` mean that scope `s` equals or descends from `r`. For every
open region `r`, the scheduler invariant is

```text
live(r) = |{ t : t is not terminal, t is not owner(r), ancestor(home(t), r) }|.
```

Spawn increments `live` along the scope ancestry chain only after frame
admission and capability transfer are ready to commit. Task completion or
cancellation decrements it once. `scope.join r` returns Ready exactly when
`live(r) = 0`; otherwise it registers one wait and parks. The optional runtime
verifier recomputes this count. Scope slots are reused with generation tags,
so a stale host token cannot join a new scope. This is checked Rust runtime
behavior and bounded tests, not a proof of the future generated code.

## Cost and erasure

`scope.enter` and `scope.join` are visible Core effects and cost sites when
the body can spawn. A scope whose body and transitively called functions
cannot spawn may be erased after an effect summary proves that fact. This is
required for the `empty_scope` cost fixture: it must compile to a direct
return without scheduler calls. The effect summary must include calls reached
from the body, not only textual `spawn` statements. Join parking is a dynamic
event; the static ledger identifies the possible join site rather than
claiming every execution parks.

## Evidence still needed

- Source tests for nested scopes, early `?`, capability cleanup, and a task
  spawned before an inner scope.
- Core verifier tests for unmatched, duplicated, and branch-dependent joins.
- Interpreter, JIT, and standalone differential runs across Ready, Pending,
  child failure, and allocation failure paths.
- Frame analysis proving values and places used after a pending join survive
  the suspension; generated code must retry the join rather than re-entering.
- A bounded formal model of nested scope membership and cancellation, with a
  separate argument for the compiler's stack and backend erasure rules.
