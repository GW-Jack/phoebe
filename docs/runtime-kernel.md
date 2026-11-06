# Cooperative runtime kernel

Status: executable Rust kernel for one scope, bounded `I64` and request
channels, and one-shot `ReplyTo<I64>`. Source syntax, Core IR operations,
compiler-generated continuations, native linking, and general typed messages
are not implemented here yet.

`Scheduler` owns a bounded table of task records and channels. Each task has
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
The task table and ready queue have explicit limits. Channel queues reserve
their full capacity before use. Rust `Box` frame allocation and the host
allocator are not yet fallible in this implementation, so this does not meet
the release requirement for safe allocation failure. The public Rust `Task`
trait also cannot by itself prevent a handwritten Rust task from sharing
mutable memory; Phoebe's future type checker and frame lowering must enforce
that boundary.

The compiler integration target is a generated poll function with a typed,
scope-owned frame. The frame retains task-local values and places across
suspension; its capability slots are included in failure cleanup metadata.
Typed channel slots need a compiler-provided layout and structural cleanup
table before arbitrary source-language messages can carry `ReplyTo` or other
capabilities. The current request layout is hard-coded to an `I64` payload and
an `I64` reply. A native runtime ABI must then connect Cranelift JIT and
standalone builds to this kernel. None of those steps is established by the
current runtime tests.
