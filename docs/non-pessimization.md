# Non-pessimization contract — draft 0.2

Phoebe should hand Cranelift a direct representation of the program, without
introducing avoidable copies, allocations, checks, scheduler calls, or dynamic
dispatch. Cranelift remains responsible for instruction selection, register
allocation, and ordinary machine-level optimization. This contract is about
costs introduced by **Phoebe's own lowering**, not a promise of globally
minimal machine code.

## 1. Cost events in checked IR

The checked IR records these operations before they become low-level calls or
memory instructions:

| Event | Meaning | Required provenance |
| --- | --- | --- |
| `copy.value` | Independent copy of a fixed-size nominal value | Explicit `copy(x)` |
| `copy.bytes` | Copy a variable-size value | Explicit `copy`, channel send/receive, or another named value-semantic boundary |
| `storage.grow` | Obtain storage for a growing task-local value | Constructed value and owning region |
| `check.bounds` | Check an index against a collection length | Indexed access and the absence of a range proof |
| `task.create` | Create a child task and retained frame | Executed `spawn` |
| `task.park` | Yield a task waiting for a channel or scope | Blocking operation that was not immediately ready |
| `task.yield` | Requeue the current task | Explicit source `yield;` or a budgeted `while` backedge |
| `scope.enter`, `scope.join` | Establish and join a lexical child lifetime | Source `scope` whose body may spawn |
| `channel.op` | Create, fork, send, receive, or close a channel capability | Corresponding source operation |
| `call.indirect` | Call through a dynamic function value or generic trait dictionary | Source-level dynamic call or trait bound |

Every event has a source span, a semantic reason, and the owning task/region
where relevant. Costful IR constructors require a reason derived from a source
operation. The core lowering and pass rules should be proved once to preserve
the event budget for every well-typed input. Full provenance and IR validation
run in compiler development and CI; they are optional in a normal user build.
The normal compiler still creates the needed IR operations, but runs no cost
solver or per-program proof search. See `compiler-architecture.md` for the
proposed proof boundary and its limits.

These are logical events, not exact counts of memory instructions. A
`copy.value` may be carried entirely in registers. The current executable
compiler emits this event for explicit nominal copies, `task.create` for
source `spawn`, and `task.yield`
for explicit yields and automatic loop checkpoints. Its static site ledger is available with
`phoebe cost <file>`; it does not count dynamic executions.
A `storage.grow` may reuse a reserved task-local page without an OS allocation.
A `channel.op` may take a fast path without parking. A backend may emit several
instructions for one event or eliminate instructions when semantics allow it.

## 2. Source-level cost rules

1. Scalars and references copy by value. The current executable compiler
   conservatively requires explicit `copy` for every nominal type, including
   small fixed-size aggregates. The eventual small-aggregate threshold is
   still open. Cranelift may remove redundant machine moves.
2. Variable-size values do not copy at assignment, ordinary call, or return.
   An owning parameter moves them; `&T` and `&mut T` borrow them explicitly.
   Fresh results are constructed in the destination. An independent copy uses
   `copy(x)`.
3. `send` copies closed value components and moves linear communication
   capabilities into queue ownership. Buffered send constructs or copies
   closed data directly into queue storage; receive copies that data into
   the receiver's task-owned storage. There is no intermediate variable-size
   temporary. A direct handoff may copy straight to the receiver once. A
   rejected send queues nothing and returns the message with its capabilities
   still owned by the caller. This is a lowering target, subject to runtime
   validation.
4. A reference is just an address plus any representation metadata required
   for safety. Creating or passing one does not register it, increment a
   reference count, or call the runtime.
5. Bounds checks remain where the index is not proven valid. Phoebe performs
   cheap, local range reasoning while region and collection facts are still
   visible; it does not require an SMT solver on the normal compile path.
6. Ordinary functions use direct native calls. Functions that may block or
   reach a loop checkpoint need retained task state. When the source program
   has no `spawn`, its loops have no automatic scheduler bookkeeping. Source
   direct calls and spawned children form a call graph; functions in a
   component containing `spawn` check a loop-local budget and yield after
   1,024 completed backedges. An unrelated source component stays direct.
   Shared helpers can make this test conservative. An empty scope
   without a loop requires no scheduler operation. A standalone build emits
   only functions reachable
   from `main`; dead source functions are still checked but do not bring
   runtime objects or a runtime entry wrapper into the binary.
7. No universal bookkeeping is inserted on every function entry, object
   access, or task-local assignment. Runtime work belongs at the operation
   that needs it.
8. `?` emits only the normal return and lexically required closes. A `with`
   cleanup has a source-level scope reason for each `channel.op` close;
   already moved or closed capabilities require no second close.
9. Generic bodies are shared by ABI shape initially. Trait calls that require
   dictionaries are visible as `call.indirect`; concrete calls remain direct.
   A later runtime-optimized mode may specialize selected generic calls.
10. `close_caps()` is structural: only owned, active communication-capability
    fields produce close operations. Capability-free values introduce no
    cleanup call. A named `send_or_close` may close a rejected message; an
    ordinary rejected `send` must return it without hidden cleanup work.

## 3. Acceptance examples

The source programs and pseudo IR in `examples/cost/` are design fixtures.
`identity.phoebe` and `empty_scope.phoebe` are executable today:
`tests/cost_acceptance.rs` checks their Core IR and the pre-legalization
Cranelift IR exposed by `phoebe clif <file>`. The remaining fixtures are
**not executable tests yet**.
That test file also checks that unreachable channel and spawn code does not
change a standalone `main` build's runtime ABI.
A future compiler test will compare
checked-IR events and selected Cranelift IR shapes, then use runtime counters
and disassembly for representative programs. Counts are per executed path,
not static instruction totals; machine-level register moves are not counted.

| Program | Required IR evidence | Forbidden Phoebe-introduced cost |
| --- | --- | --- |
| `identity` | Direct scalar return | Any listed cost event or runtime call |
| `borrow_length` | Borrow and length read | `copy.bytes`, `storage.grow`, reference bookkeeping |
| `build_one` | Construct result in its destination; one logical list growth | Intermediate `copy.bytes` on return |
| `checked_index` | One `check.bounds` for unconstrained `i` | Duplicate checks for the same access on a path |
| `range_loop` | Range fact proves each loop index; no `check.bounds` in the body | A per-iteration bounds check |
| `empty_scope` | Direct return | `task.create`, `task.park`, scheduler call |
| `send_value` | One source-to-queue `copy.bytes` on buffered success | Extra full-size temporary copy; copy on rejected send |

The last row does not count a later receive-side copy. That is a separate
operation and is necessary under the initial task-local ownership model.

## 4. Verification path

The executable compiler emits a tab-separated static cost-site ledger on
demand with `phoebe cost <file>`. It currently records `copy.value`,
`task.create`, `task.yield`, `scope.enter`, and `scope.join`, with a distinct
reason for an automatic loop checkpoint. Scope events also arise from source
`scope` statements that may spawn. Empty-scope erasure is an executable
acceptance check.
The development compiler should extend it to the remaining event kinds.
Acceptance tests compare it with these fixtures. An IR verifier checks
provenance and forbidden operations in tests and debug builds. Proofs of core
lowering and reusable rewrite rules are checked when the compiler changes, not
when user programs compile. Differential tests compare the IR interpreter and
native code's observable results. Microbenchmarks and generated-code
inspection test whether runtime and backend implementations meet the intended
cost shape.
`docs/performance-baseline.md` records the first local phase-timing snapshot
and its narrow interpretation.

The existing Maude models check safety properties only. A later costed
transition model could check that source operations and IR traces agree on
copy and channel events. Such a model would still not prove that Cranelift
emits the fewest possible instructions.
