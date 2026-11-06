# Non-pessimization contract — draft 0.1

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
| `copy.bytes` | Copy a variable-size value | Explicit `copy`, channel send/receive, or another named value-semantic boundary |
| `storage.grow` | Obtain storage for a growing task-local value | Constructed value and owning region |
| `check.bounds` | Check an index against a collection length | Indexed access and the absence of a range proof |
| `task.create` | Create a child task and retained frame | Executed `spawn` |
| `task.park` | Yield a task waiting for a channel or scope | Blocking operation that was not immediately ready |
| `channel.op` | Create, fork, send, receive, or close a channel capability | Corresponding source operation |
| `call.indirect` | Call through a dynamic function value | Source-level dynamic call or dispatch |

Every event has a source span, a semantic reason, and the owning task/region
where relevant. A pass that introduces an event must attach its reason; a pass
that removes an event records the proof or transformation that made it
unnecessary. Provenance is diagnostic metadata and may be erased after audit.
The IR verifier rejects an unaccounted event. It also rejects a variable-size
copy at an ordinary assignment, argument pass, or return where no explicit
copy or value-semantic boundary exists.

These are logical events, not exact counts of memory instructions. A
`storage.grow` may reuse a reserved task-local page without an OS allocation.
A `channel.op` may take a fast path without parking. A backend may emit several
instructions for one event or eliminate instructions when semantics allow it.

## 2. Source-level cost rules

1. Scalars, references, and small fixed-size aggregates copy by value.
   Larger fixed-size aggregates need an explicit `copy` or a reference
   parameter. The small-aggregate threshold is still open. Cranelift may
   remove redundant machine moves.
2. Variable-size values do not copy at assignment, ordinary call, or return.
   Same-task calls borrow them, and fresh results are constructed in the
   destination. An intentional independent copy uses `copy(x)`.
3. `send` is an explicit value-copy boundary. Buffered send constructs or
   copies directly into queue storage; receive copies into the receiver's
   task-owned storage. There is no intermediate variable-size temporary.
   A direct handoff may copy straight to the receiver once. A rejected send
   queues nothing. This is a lowering target, subject to runtime validation.
4. A reference is just an address plus any representation metadata required
   for safety. Creating or passing one does not register it, increment a
   reference count, or call the runtime.
5. Bounds checks remain where the index is not proven valid. Phoebe performs
   cheap, local range reasoning while region and collection facts are still
   visible; it does not require an SMT solver on the normal compile path.
6. Ordinary functions use direct native calls. Only functions that may block
   need retained task state. An empty scope requires no scheduler operation.
7. No universal bookkeeping is inserted on every function entry, object
   access, or task-local assignment. Runtime work belongs at the operation
   that needs it.

## 3. Acceptance examples

The source programs and pseudo IR in `examples/cost/` are design fixtures.
They are **not executable tests yet**. A future compiler test will compare
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

The first compiler implementation should emit a machine-readable cost ledger
alongside checked IR. Acceptance tests compare that ledger with these fixtures.
The IR verifier checks provenance and forbidden operations after each lowering
pass. Differential tests compare the IR interpreter and native code's
observable results. Microbenchmarks and generated-code inspection then test
whether runtime and backend implementations meet the intended cost shape.

The existing Maude models check safety properties only. A later costed
transition model could check that source operations and IR traces agree on
copy and channel events. Such a model would still not prove that Cranelift
emits the fewest possible instructions.
