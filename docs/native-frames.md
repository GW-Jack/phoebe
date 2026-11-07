# Retained native task frames

Status: the spill analysis and `phoebe frames` inspection command run on
verified Core. The runtime can allocate and poll a stable native frame through
the versioned v1 ABI. Generated Cranelift poll functions and native task spawn
are not implemented yet. The analysis is tested, not formally proved.

## Suspension boundary

A compiled task must never restart a source function after `Pending`. A send
or receive retries its terminator with the original message or endpoint;
instructions earlier in the block have already run. A synchronous call to a
suspendable callee retains the callee frame and resumes that frame before
continuing the caller. Reinvoking the call would repeat ownership transfer
and any earlier effects.

Let `S₀` be functions containing `channel.send.i64` or
`channel.receive.i64`. The set of suspendable functions is the least fixed
point `S = S₀ ∪ {f | f calls g and g ∈ S}`. `spawn` does not place its parent
in `S`: the child is a separate task. A function outside `S` keeps its
ordinary direct calling convention. Runtime-context use is a different
property; channel creation and spawn need a context but do not by themselves
need a retained frame.

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
The union of point-local sets determines the function's payload slots. A
call-only frame may have no payload slots but still needs a continuation
header and child-frame link.

`phoebe frames <file|project>` prints those point-local and union sets. For
`examples/channel_exchange.phoebe`, it shows two send points in `producer`,
two receive points in `exchange`, and a call continuation in `main`.
`examples/spawn_close.phoebe` needs no retained frame. This is a planned
layout, not yet native code generation.

## Native ABI obligations

The v1 poll ABI uses status `0` for Ready, `1` for Pending, and `255` for fault.
The frame is a stable array of `I64` words: word 0 is the program counter,
word 1 is the result written only on Ready, and later words are the generated
payload layout. The runtime allocates and zeroes it before execution and
keeps it in the task record across Pending. The standalone entry point
`phoebe_rt_v1_main_poll` runs a zero-argument poll function under a bounded
scope. A synthetic poll test yields and resumes at the same frame address;
another parks in the versioned receive ABI, wakes from a transferred sender,
retries receive, and closes its endpoint.

Generated code still needs a program counter that distinguishes
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
for all Core programs or the future native implementation. A bounded model
and differential tests should check the poll ABI after it exists. The
compiler should run the established analysis, not execute a model checker
for each build.
