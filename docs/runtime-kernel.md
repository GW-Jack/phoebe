# Cooperative runtime kernel

Status: executable Rust kernel for one scope, bounded `I64` and request
channels, a fixed-width typed-message queue, and one-shot `ReplyTo<I64>`.
Checked Core programs interpret
channel creation, traffic, endpoint close, and task spawn through it. Cranelift
JIT and standalone object code call the same kernel through a C ABI shim.
Source syntax covers fallible `I64` channel creation, explicit endpoint
close, task spawn, and `I64` send/receive outcomes. Native call continuations
run in the supported poll subset. Concrete typed messages with scalar fields
and `ReplyTo<I64>` leaves now use the same kernel in interpreter, JIT, and
standalone paths.

The Rust `typed_words_channel` and `typed_right_channel` APIs reserve a flat
ring of `capacity × width` words when the channel is created. Send and receive
check a concrete `type_id` and width before touching the queue. A zero-width
value still occupies one slot. The latter API stores a tag-aware plan for
`ReplyTo<I64>` leaves. Enqueue validates every active right and duplicate
before changing any owner; dequeue returns those rights to the receiving task.
A full or closed send leaves the caller's words and rights untouched. Closing
the last receiver structurally closes queued rights. Tests cover multiple
rights, tagged inactive payloads, rejection, discard, and two-task parking.
For an `Empty` right plan, successful typed send and receive skip the
right-collection walk; type identity, width, capacity, and queue order checks
still apply. `docs/performance-baseline.md` records a local scalar/typed
comparison before and after that path was introduced.
The existing checked Core `Request<I64>` operations now use the typed queue
through the request runtime adapters, including in JIT and standalone output.
Descriptor generation for a checked ABI layout is available in
`phoebe_native`. Versioned native typed-channel shims now create a queue from
that descriptor and send or receive flattened words. A host ABI witness moves
a reply right through the queue and checks closed-send retention. Generated
Cranelift calls now cover concrete `channel<T>` messages in the supported
layout subset. Channel handles and other transferable right kinds, plus the
remaining failure-edge comparisons, are next steps in
[`typed-messages.md`](typed-messages.md).

`Scheduler` owns bounded tables of task records, lexical scope slots,
channels, handles, and reply slots. The caller supplies all five limits. Each
task has one retained `Task` frame and is polled until it returns `Ready` or registers a
wait and returns `Pending`. The scheduler runs on one OS thread. A full send
parks the sender with the original message still in its frame; a receive of an
empty channel parks the receiver. Queue changes wake a waiting task. A
one-slot channel never holds more than one message, and no task needs an OS
thread while parked. `yield_now` requeues a runnable task without treating it
as a blocked channel operation. Source `yield;` lowers to a retained
continuation in the interpreter and native ABI. The compiler inserts bounded
backedge checkpoints in source call-graph components connected to `spawn`, so
a CPU-heavy sibling can yield without an explicit source statement.
The default source-run scope permits 64 tasks, 64 nested scope slots, 64
channels, 256 total channel handles, and 64 reply pairs; callers can set
tighter limits through the Rust API. Task creation
therefore succeeds in the ordinary interpreter CLI example, while a one-task
limit exercises the typed failure edge.
The Rust kernel also has lexical task groups: `scope_enter()` switches the
calling task's spawn destination to a fresh scope, and `scope_join(scope)`
waits for every task spawned there or in a nested scope. It does not wait for
siblings started before entry. The owner must join innermost first and cannot
finish successfully while an entered scope remains open. Scope slots are
reused with generation-tagged tokens, so an old token cannot join a later
scope. The kernel checks live task counts and scope ancestry in its optional
verifier. A scope entry at capacity faults before changing the task's current
scope. Closed slots can be reused at the limit. Source `scope { ... }` lowers
through these operations when its body may spawn.
After a fault or deadlock cancels every task, the scheduler retires any open
nested scope slots. The verifier rejects an active nested scope whose owner
has already terminated. Runtime tests cover an open scope with an unpolled
child on fault and an open scope with a parked receiver on deadlock.
The native archive exports `phoebe_rt_v1_scope_enter` and
`phoebe_rt_v1_scope_join`. Join returns Ready or Pending and is retried after
the scheduler wakes the task. A synthetic native poll task exercises entry,
spawn, park, and join through those calls; an unmatched join faults. Scope
record allocation failure currently faults the task. Core checks balanced
scope operations and carries source spans through entry and join.
The versioned `phoebe_rt_v1_main_poll` entry runs a native poll function with
a stable word-array frame: program counter in word 0, result in word 1, and
generated payload afterward. Status `0` means Ready, `1` Pending, and `255`
fault. Synthetic poll tests prove the runtime driver preserves its frame
address across yield and a channel receive wake. Generated Cranelift now uses
this ABI for scalar channel traffic, same-task call continuations, and spawned
poll children in the source exchange.

`Sender` and `Receiver` wrap opaque tokens. The handle table records each
token's one task owner. `fork` creates another handle explicitly and returns
a typed handle-limit or allocation failure before changing channel counts or
publishing a handle; invalid tokens remain internal faults. Handle tokens are
never reused in a scope, so the handle budget counts total minted handles,
including closed ones. Source binds the new right
with `val another = sender.fork()?` or `receiver.fork()?`.
Both fork kinds use one pre-commit reservation path. A kernel test injects a
failed handle-table reservation and verifies that the original handles, table
length, channel endpoint counts, and later successful forks remain valid. It
does not force host allocator failure in generated native code.
`spawn` validates all transfers before changing their owners. The Rust
`spawn_capabilities_owned` entry also transfers reply rights and reply
receivers, returning the child frame intact on failure. A successful send
copies a closed `I64` into a pre-reserved FIFO slot. If no receiver remains,
`send` returns `Closed(message)` without queueing or consuming it. Closing
the last receiver discards queued `I64` values and wakes blocked senders;
closing the last sender wakes receivers to observe `End` after draining.
Scalar sends and receives cannot operate on request channels.

Compiler-generated frames should use `spawn_owned` and
`send_request_owned`. On a validation, capacity, or waiter-allocation error,
these methods return the original child frame or request before transferring
any capability. The older `spawn` and `send_request` convenience methods
convert those errors to strings for Rust tasks that fail immediately; they
discard the wrapper, after which task-failure cleanup closes any table-owned
rights. A generated task must not use those convenience methods for a
recoverable source-language failure edge. Runtime validation still cannot
inspect arbitrary fields of a handwritten Rust child frame; checked Core IR
must supply and verify its transfer list.

Channel and reply creation now also have typed kernel entry points:
`channel_typed`, `request_channel_typed`, and `reply_typed` return
`ResourceError` variants for invalid capacity, table limits, and allocation
failure. The existing string-returning calls are wrappers for handwritten
Rust tasks. All capacity reservations precede publication of either new
right, so an error creates no partial endpoint pair. Core lowers hand-built
`channel.new.i64` edges through `channel_typed` in its interpreter, and its
verifier checks the success/failure ownership graph. Source syntax and native
code generation use this bridge for creation and close. The Core interpreter
now retains call frames and places across pending scalar sends and receives.
Core `spawn` now creates a child task and transfers its checked arguments on
success. A test runs a Core producer and parent receiver through the scheduler.
Source task creation runs in the interpreter and native code for both direct
and suspendable children. Generated native same-task calls retain their
callee frames across waits. Borrowed caller places remain in the parent
frame until the callee returns; a deadlocked borrowed call is tested against
the scheduler's cleanup and ownership verifier.
Pending native same-task callees are released from the leaf inward when their
task frame is dropped, before the parent frame. A deeper borrowed-call
deadlock regression checks interpreter, JIT, standalone, and terminal table
cleanup across this unwind path.

`reply()` creates a one-shot right and receiver. A `Request` holds an `I64`
value and that right. `send_request` on a full or closed queue returns the
intact request to its task; a successful send changes the right's owner to the
queue. `receive_request` changes ownership to the receiving task. `complete`
consumes the right, delivering one value or returning `Closed(value)` if the
reply receiver was canceled. Closing the last request receiver discards queued
requests and closes their rights. Failure after dequeue closes rights owned by
the failed task. The reply receiver observes `End` when its right closes
without completion. These transitions are checked by the runtime tests and
the optional table verifier.

Normal task completion with an open handle or reply capability is a task
failure. Task failure or panic closes that task's owned capabilities, then
cancels every remaining task in the scope before another task poll. Spawn IDs
increase from parent to child; failure and deadlock cleanup retire live frames
in descending ID order. When a task itself fails, its detached frame is
dropped after younger frames and before older frames. Drop-order tests cover
failure in a root or child and deadlock across three generations. The first
failure is reported to the host; cancellation closes siblings' capabilities
and discards queued messages whose final receiver closes. A root waiting on
its own still-open sender therefore cannot mask a child fault as a deadlock.
Detected scope deadlock also closes all task-owned capabilities. A task can
join its entire descendant tree while unrelated sibling tasks remain active;
the root joins all tasks. Spawn records each child's parent, and completion
wakes a joiner only after its descendants have finished. `scope_join` uses
scope membership instead of task ancestry. A child fault still
fails the whole scope immediately. Source `scope { ... }` exposes lexical
nested task scopes; task completion without an explicit join still leaves
its descendants under the root scope.
The optional `Scheduler::verify` scans handle counts, queue bounds, reply
owners, message shapes, and waiter registrations in tests; ordinary polling
does not run that scan.

The kernel allocates bounded channel storage and retained frames under scope
ownership, without a garbage collector or a general shared program heap.
The task, channel, handle, and reply tables have explicit limits. Channel queues reserve
their full capacity before use. Task body boxing now uses a fallible global
allocation before any spawn transfer commits; allocation failure returns the
intact child and a typed `ResourceError::Allocation`. An injected allocation
failure test exercises this scheduler path and checks that the original child
and its channel right remain with the parent. Other host allocations,
including error strings, still assume the Rust allocator can provide memory;
safe handling of every possible host OOM remains open. The public Rust `Task`
trait also cannot by itself prevent a handwritten Rust task from sharing
mutable memory; Phoebe's future type checker and frame lowering must enforce
that boundary.
Reply slots are not reused yet, so the reply limit bounds the total number
created in one scope rather than only the number simultaneously open.

Generated poll functions use typed, scope-owned frames to retain task-local
values and places across suspension. For the current concrete rights, the
runtime ownership tables close all rights still owned by a failed or canceled
task. A compiler-derived frame cleanup table remains a target for general
typed messages and resource values that the runtime cannot enumerate.
Typed channel slots need a compiler-provided layout and structural cleanup
table before arbitrary source-language messages can carry `ReplyTo` or other
capabilities. The current request layout is hard-coded to an `I64` payload and
an `I64` reply. The native ABI also exports `phoebe_rt_v1_send_i64` and
`phoebe_rt_v1_receive_i64`. Send reports `Sent`, `Pending`, or `Closed`;
receive reports `Value`, `Pending`, or `End` and writes its output word only
on `Value`. Both calls retain the endpoint. Runtime tests exercise pending
sends and receives across scheduler wakeups. Cranelift-generated poll code
uses these shims, returning to the scheduler on `Pending` and retrying after
a wake.
The native ABI also exports `reply_new_i64`, `reply_complete_i64`,
`reply_receive_i64`, `reply_close_i64`, and `spawn_poll_caps` calls. The last
accepts typed descriptors for channel handles, reply rights, and reply
receivers, and commits all transfers together. Synthetic native poll tests
cover pair creation, one-shot completion, cancellation, resource failure,
a parked reply receiver waking after a cross-task transfer, and native spawn
success and task-limit rejection. Checked Core and source lowering use the
Rust kernel operations through the interpreter and generated Cranelift code.
Source tests cover reply delivery across a spawned task, cancellation,
lexical close, and resource failure in the interpreter, JIT, and standalone
executable. Checked Core now executes the runtime's concrete
request channel and request value, transferring the nested `ReplyTo` through
enqueue, dequeue, and unpack. The interpreter, JIT, and standalone executable
agree on the concrete request layout. Source programs expose the `I64`
request layout through structural send and receive arms; arbitrary messages
carrying `ReplyTo` remain open.
The native ABI now exports `request_channel_new_i64`, `send_request_i64`,
and `receive_request_i64`. The concrete request is two I64 words: value and
reply-right token. Send returns `Sent`, `Pending`, or `Closed`; only `Sent`
transfers the right to the queue. Receive writes both words only on `Value`,
retaining the receiver on `Pending`. Synthetic native poll tests cover a
round trip, closed-send ownership retention, a waiting service's wakeup, and
creation failure without partial output. Generated Cranelift request code
uses these shims for the concrete two-word request layout.
The C ABI creation call publishes both output tokens only on
success; native generated code loads them only on that edge. Standalone builds
link `libphoebe_runtime.a`. Buck2 supplies it through the CLI's resource map;
a Cargo development build can produce it with
`cargo build -p phoebe-runtime --lib` when it is missing. A distributed
compiler must include the matching archive.
The exported symbols carry a `phoebe_rt_v1_` prefix, so an archive with a
different ABI version fails at link time.
The standalone wrapper uses `phoebe_rt_v1_main_diagnostic` or
`phoebe_rt_v1_main_poll_diagnostic` to receive a bounded error string and
print the scheduler's failure reason. The original entry symbols remain
available for existing hosts. A source deadlock test checks the standalone
stderr against the interpreter and native scheduler reason.

The creation status is `0` on success, `1` for invalid capacity, `2` for the
channel limit, and `4` for allocation failure (`3` is reserved for the reply
limit, `5` for the task limit, and `6` for the handle limit). `255` marks an internal ABI fault; the native task reports that fault
after the generated function returns. These codes are an internal bridge,
not a stable source-language error representation.
