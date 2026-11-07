# Cooperative runtime kernel

Status: executable Rust kernel for one scope, bounded `I64` and request
channels, and one-shot `ReplyTo<I64>`. Hand-built Core programs can interpret
channel creation and endpoint close through it. Cranelift JIT now calls the
same kernel for those operations through a C ABI shim. Source syntax,
compiler-generated continuations, standalone native linking, and general
typed messages are not implemented here yet.

`Scheduler` owns bounded tables of task records, channels, and reply slots.
The caller supplies all three limits. Each task has
one retained `Task` frame and is polled until it returns `Ready` or registers a
wait and returns `Pending`. The scheduler runs on one OS thread. A full send
parks the sender with the original message still in its frame; a receive of an
empty channel parks the receiver. Queue changes wake a waiting task. A
one-slot channel never holds more than one message, and no task needs an OS
thread while parked. `yield_now` requeues a runnable task without treating it
as a blocked channel operation. Compiler-generated backedge checkpoints are
still needed to make CPU-heavy Phoebe loops fair.

`Sender` and `Receiver` wrap opaque tokens. The handle table records each
token's one task owner. `fork` creates another handle explicitly; `spawn`
validates all transfers before changing their owners. A successful send
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
code generation still need this bridge. The current Core interpreter uses one
root task; send, receive, child tasks, and suspension are not yet executable
through Core.

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
failure. Task failure, panic, and detected scope deadlock close that task's
owned capabilities, then wake affected peers. The root can join its child
tasks and observe a child failure.
The optional `Scheduler::verify` scans handle counts, queue bounds, reply
owners, message shapes, and waiter registrations in tests; ordinary polling
does not run that scan.

The kernel allocates bounded channel storage and retained frames under scope
ownership, without a garbage collector or a general shared program heap.
The task, channel, and reply tables have explicit limits. Channel queues reserve
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
an `I64` reply. The JIT ABI currently covers only scalar channel creation and
endpoint close. The C ABI creation call publishes both output tokens only on
success; native generated code loads them only on that edge. The standalone
linker does not yet include this runtime ABI.

The creation status is `0` on success, `1` for invalid capacity, `2` for the
channel limit, and `4` for allocation failure (`3` is reserved for the reply
limit). `255` marks an internal ABI fault; the JIT task reports that fault
after the generated function returns. These codes are an internal bridge,
not a stable source-language error representation.
