# Lexical task scopes: Core contract

Status: source statement, checked Core representation, and interpreter/native
execution implemented for the current subset. The full release invariant is
still under test.

`scope { body }` creates a child lifetime inside the current task. Every task
spawned while that body runs, including descendants spawned by those tasks,
belongs to the scope. Leaving the body waits for that set to finish. Tasks
started before the block remain in the enclosing scope and do not delay this
join. A child fault cancels the outer runtime scope, including siblings;
ordinary completion closes only the lexical scope. The first source form is a
statement. A later expression form may carry a result through the join.

## Checked IR representation

Core uses an effectful `scope.enter r` instruction and
a `scope.join r -> resume` terminator. `r` is a function-local region ID,
not a transferable runtime value. A join may park and retry the same
terminator. The enter instruction and each join carry source spans; the
verifier requires one static enter per region ID. Generated code
calls `phoebe_rt_v1_scope_enter` and `phoebe_rt_v1_scope_join`. The scheduler
keeps the current scope in the task record, so the region ID needs no native
frame word or channel message representation.

The Core verifier propagates a stack of region IDs through every block. An
enter pushes its ID; a join must match and pop the top ID on its resume edge.
All incoming edges to a block must agree on the complete stack. A return
requires an empty stack, and a region cannot be entered twice while already
open. This checks LIFO closure through branches, loops, calls, and `?`
without running a proof search in ordinary compilation. Source lowering routes
implicit `?` returns through joins in reverse lexical order, after closing
rights still owned by the returning task. A failed scope entry faults the
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
cannot spawn is erased after a directed call effect summary. The
`empty_scope` cost fixture compiles without scheduler calls. Incremental Core
reuse checks called functions' spawn effects before retaining a prior body.
Join parking is a dynamic
event; the static ledger identifies the possible join site rather than
claiming every execution parks.

## Evidence still needed

- Source tests cover nested scopes, early `?`, a child already running at
  failure, transitive and imported calls, erasure, and a sibling spawned
  before the inner scope. Broader cancellation paths remain untested.
- Core verifier tests for more complex loops; unmatched, duplicated, and
  branch-dependent joins are already rejected.
- Differential runs across child failure and allocation failure paths. A
  hand-built Core program already covers Ready, Pending, and a spawn-limit
  failure in the interpreter and JIT, plus Pending in standalone code.
- More frame tests for borrowed places across a pending join. Existing
  scalar and mutable-place tests show that generated code retries the join
  rather than re-entering.
- The finite `formal/lexical_scopes.maude` model checks transitive membership,
  sibling isolation, and fault cancellation for four fixed tasks. A separate
  argument is still needed for the compiler's region stack and backend erasure
  rules, and no refinement proof links that model to generated code.
