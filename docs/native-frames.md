# Retained native task frames

Status: the spill analysis and `phoebe frames` inspection command run on
verified Core. The runtime can allocate and poll stable native frames through
the versioned v1 ABI. Generated Cranelift poll functions handle scalar channel
retries, same-task calls into suspendable functions, channel creation, and
spawned poll children. The public `run` and standalone `build` paths select
this backend when `main` can suspend. A direct parent can spawn a suspendable
child through a separately compiled poll entry; its own calls retain the
direct ABI. A suspendable function can call an owned-value, direct-only
helper synchronously using the direct ABI; the helper gets no retained frame.
A direct helper may spawn a suspendable child even when called from a retained
function. The poll module includes that child's retained entry in the same
native closure, including when the helper is reached through other direct
helpers.
A suspendable parent can also spawn a direct-only child through a small
poll-compatible wrapper; the child body stays on the direct ABI.
The object path uses the same poll lowering as the JIT;
standalone exchange, closed-send, and mixed-spawn tests execute linked binaries.
The analysis is tested, not formally proved.

## Suspension boundary

A compiled task must never restart a source function after `Pending`. A send
or receive retries its terminator with the original message or endpoint;
instructions earlier in the block have already run. A synchronous call to a
suspendable callee retains the callee frame and resumes that frame before
continuing the caller. Reinvoking the call would repeat ownership transfer
and any earlier effects.

An explicit `yield;` spills live state and resumes at the following block,
without replaying the yield. Let `S₀` be functions containing
`channel.send.i64`, `channel.receive.i64`, or `yield`. The set of suspendable
functions is the least fixed
point `S = S₀ ∪ {f | f calls g and g ∈ S}`. `spawn` does not place its parent
in `S`: the child is a separate task. A function outside `S` keeps its
ordinary direct calling convention. Runtime-context use is a different
property; channel creation and spawn need a context but do not by themselves
need a retained frame.
The generated call site chooses the direct ABI when its callee is outside
`S`. A call into `S` saves a continuation and uses the native call mailbox.
`examples/channel_exchange_helper.phoebe` exercises both paths, including a
flattened `Result` returned by a direct helper after a receive and a direct
helper that creates and closes a channel through the hidden runtime context.
`examples/spawn_direct_from_poll.phoebe` parks the parent receiver until its
direct child closes the sender, then resumes on the end-of-stream edge.

## Spill equation

For a verified function, `LiveIn[b]` is the least fixed point of backward
value and place liveness. An edge to block `t` substitutes live block
parameters with the arguments supplied by a `jump`. Parameters produced by a
terminator, such as a received value or a creation error, have no old frame
value to spill. The block transfer walks its instructions backward, killing a
new SSA result or overwritten place and adding each operand or place read.

At a channel retry, the spill set is the live state immediately before the
terminator, including its endpoint and message operands. At a pending call,
the child owns its arguments and frame; the caller spills values needed
*after* the call, excluding the result that has not been produced yet. A
borrowed caller place remains in the caller frame while the child uses it.
At `yield`, the spill set is the state live in the resume block.
The union of point-local sets determines the function's payload slots. A
call-only frame may have no payload slots but still needs a continuation
header and child-frame link.

For a live scalar lexical reference at a suspension point, the spill set
also contains its transitive `borrow.begin` root place. This makes the
reference address point into the stable frame instead of a poll invocation's
stack slot. Hand-built checked Core exercises this through `yield`, channel
send, both channel receive outcomes, and an ordinary call to a suspendable
callee. Other terminator suspension operations still reject live lexical loans.

`phoebe frames <file|project>` prints those point-local and union sets. For
`examples/channel_exchange.phoebe`, it shows two send points in `producer`,
two receive points in `exchange`, and a call continuation in `main`.
`examples/spawn_close.phoebe` needs no retained frame. Generated poll code
uses this layout for the source exchange.
`examples/channel_accumulator.phoebe` has a mutable total used after two
receives, so its place lives in the stable frame. Its mutable channel capacity
dies before the first receive and uses a local stack slot instead. The example
also evaluates product fields, arithmetic, and comparisons after resumption.
`examples/borrow_across_wait.phoebe` forwards an exclusive borrow into a
callee that parks on receive, then writes through it. The caller place stays
in its stable frame until the callee returns; the callee frame retains the
reference value during the wait.
A deadlocked borrowed call is also tested through the native scheduler:
scope cleanup releases its handles and the runtime ownership verifier passes.
On task cancellation, `NativePollTask` drops pending same-task callee frames
from the leaf inward before freeing the caller frame. A nested borrowed-call
deadlock test exercises three forwarded callees in the interpreter, JIT, and
standalone executable and checks the native scheduler's terminal ownership
state. This tests the unwind path; it does not prove generated pointer safety.
`formal/retained_borrow.maude` separately explores a bounded nested-call
model, including ordered unwind after a parked failure. It does not prove
the generated frame code.

## Native ABI obligations

The v1 poll ABI uses status `0` for Ready, `1` for Pending, `3` for a queued
same-task call, and `255` for fault.
The frame is a stable array of `I64` words: word 0 is the program counter,
words starting at 1 hold the flattened result, and later words hold the
generated payload. Each entry parameter has a frame address before first
execution; a parameter live across a suspension shares its retained value
slot. Entry parameters that die before the first retry still need an initial
address, but do not enter the spill set. A root task returns `I64`, so its
result is word 1. A spawned task may return `Unit`, which has zero result
components. Its frame still reserves word 1 for scheduler completion and
receive scratch space, but generated code writes no dummy result. A same-task
call can copy zero result words through the call mailbox. A finite product
containing only zero-width fields can use the same path; its nominal ownership
is still checked in Core.
The compiler's physical layout uses the existing ABI shape widths for each
live value and place, and rejects a layout beyond fixed address offsets.
For the source exchange, `exchange` reserves two result words for
`Result<I64, ResourceError>`; the root `main` reserves one.
The runtime allocates and zeroes the root frame before execution and
keeps it in the task record across Pending. A same-task call mailbox now
accepts a fully initialized child frame after the parent saves its PC.
The scheduler polls the child, retains both frames across a yield, copies a
checked result range into the parent, and resumes the parent without
replaying its entry. Synthetic ABI tests exercise this sequence and reject a
result range outside the parent frame. A combined scheduler test parks the
child on a real channel receive, wakes it from a sender task, and then resumes
the parent. Generated Cranelift calls now use this mailbox, including a
flattened two-word `Result` across a suspended call.
The standalone entry point
`phoebe_rt_v1_main_poll` runs a zero-argument poll function under a bounded
scope. The object builder exports `phoebe_main_poll` and links the runtime
archive through this entry point. A synthetic poll test yields and resumes at the same frame address;
another parks in the versioned receive ABI, wakes from a transferred sender,
retries receive, and closes its endpoint.

The physical layout reserves PC 0 for first entry and assigns positive PCs
to suspension points in stable `(block, instruction)` order. These IDs are
compiler-internal to one build; persisted frames or cross-version resume are
outside the v1 ABI. A generated retry must start at the pending terminator or
call continuation, using only that point's retained slots, without replaying
earlier instructions in the block.

Generated code uses a program counter that distinguishes
first entry from each retry or child continuation. The scheduler owns a
pending frame until completion or cancellation. Frame initialization and
task admission must reserve storage before transferring endpoint ownership;
failed spawn leaves the parent arguments intact. On cancellation or failure,
the runtime must close exactly the task-owned rights recorded by checked Core.

The compiler must establish two correspondences for every suspension point:

1. Every value or place read after resume is either in that point's frame
   spill set or is freshly produced by the resumed operation.
2. Every live capability is held by exactly one of the parent frame, a child
   frame, or a scheduler-owned queue. The capability verifier's retry state
   and the frame's retained rights must agree.

The current liveness tests include loop backedges, dead-value exclusion,
mutable places, and transitive calls. They do not establish these obligations
for all Core programs or the full native implementation. The generated
receive leaf is tested with scheduler parking, wake, value delivery, and end
of stream. It retains a scalar computed before the receive while an entry
parameter dies before retry. A generated two-send leaf tests one-slot
backpressure, retry, and the closed edge returning the original unsent
message. Its pre-legalization CLIF is available from the compiled artifact.
The versioned `phoebe_rt_v1_spawn_poll` ABI initializes a child frame before
committing endpoint transfer. Tests use a generated send leaf to cover
successful admission and a task-limit failure that leaves the rights with
the parent. Generated code now calls this ABI for a spawned poll child in the
source exchange.
A broader differential suite and bounded model should check subsequent poll
paths. The compiler should run the established analysis, not execute a model
checker for each build.
