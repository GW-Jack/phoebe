# Cooperative runtime kernel

Status: executable Rust kernel for one scope, bounded `I64` and request
channels, and one-shot `ReplyTo<I64>`. Checked Core programs interpret
channel creation, traffic, endpoint close, and task spawn through it. Cranelift
JIT and standalone object code call the same kernel through a C ABI shim.
Source syntax covers fallible `I64` channel creation, explicit endpoint
close, task spawn, and `I64` send/receive outcomes. Native call continuations
run in the supported poll subset; general typed messages remain open.

`Scheduler` owns bounded tables of task records, channels, handles, and reply slots.
The caller supplies all four limits. Each task has
one retained `Task` frame and is polled until it returns `Ready` or registers a
wait and returns `Pending`. The scheduler runs on one OS thread. A full send
parks the sender with the original message still in its frame; a receive of an
empty channel parks the receiver. Queue changes wake a waiting task. A
one-slot channel never holds more than one message, and no task needs an OS
thread while parked. `yield_now` requeues a runnable task without treating it
as a blocked channel operation. Source `yield;` now lowers to a retained
continuation in the interpreter and native ABI. Compiler-generated backedge
checkpoints are still needed to make CPU-heavy Phoebe loops fair without an
explicit yield.
The default source-run scope permits 64 tasks, 64 channels, 256 total
channel handles, and 64 reply pairs; callers can set tighter limits through
the Rust API. Task creation
therefore succeeds in the ordinary interpreter CLI example, while a one-task
limit exercises the typed failure edge.
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
with `val another = sender.fork()?` or `receiver.fork()?`. `spawn`
validates all transfers before changing their owners. The Rust
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
cancels every remaining task in the scope before another task poll. The first
failure is reported to the host; cancellation closes siblings' capabilities
and discards queued messages whose final receiver closes. A root waiting on
its own still-open sender therefore cannot mask a child fault as a deadlock.
Detected scope deadlock also closes all task-owned capabilities. The root
can join successful children; a child fault fails the scope immediately.
The optional `Scheduler::verify` scans handle counts, queue bounds, reply
owners, message shapes, and waiter registrations in tests; ordinary polling
does not run that scan.

The kernel allocates bounded channel storage and retained frames under scope
ownership, without a garbage collector or a general shared program heap.
The task, channel, handle, and reply tables have explicit limits. Channel queues reserve
their full capacity before use. Rust `Box` frame allocation and the host
allocator are not yet fallible in this implementation, so this does not meet
the release requirement for safe allocation failure. The public Rust `Task`
trait also cannot by itself prevent a handwritten Rust task from sharing
mutable memory; Phoebe's future type checker and frame lowering must enforce
that boundary.
Reply slots are not reused yet, so the reply limit bounds the total number
created in one scope rather than only the number simultaneously open.

The compiler integration target is a generated poll function with a typed,
scope-owned frame. The frame retains task-local values and places across
suspension; its capability slots are included in failure cleanup metadata.
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
success and task-limit rejection. Hand-built checked Core uses the Rust
kernel operations through its interpreter and generated Cranelift code. Source
lowering remains open, so these tests do not establish source-level `ReplyTo`
support.
The C ABI creation call publishes both output tokens only on
success; native generated code loads them only on that edge. Standalone builds
link `libphoebe_runtime.a`. Buck2 supplies it through the CLI's resource map;
a Cargo development build can produce it with
`cargo build -p phoebe-runtime --lib` when it is missing. A distributed
compiler must include the matching archive.
The exported symbols carry a `phoebe_rt_v1_` prefix, so an archive with a
different ABI version fails at link time.

The creation status is `0` on success, `1` for invalid capacity, `2` for the
channel limit, and `4` for allocation failure (`3` is reserved for the reply
limit, `5` for the task limit, and `6` for the handle limit). `255` marks an internal ABI fault; the native task reports that fault
after the generated function returns. These codes are an internal bridge,
not a stable source-language error representation.
