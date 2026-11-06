# Phoebe language specification — draft 0.1

Status: design proposal, 2026-11-06. Syntax examples are illustrative. Only the
bounded core in `formal/core.maude` has an executable semantics so far.

## 1. Purpose and non-goals

Phoebe is a statically typed, native language with approachable Kotlin- and
Gleam-influenced syntax, task-local memory, and goroutine-style concurrency.
Normal compilation should be fast. Cranelift is the sole planned code generator.
The language has no garbage collector or general shared mutable program heap.

Phoebe does not promise freedom from deadlock, termination, absence of logical
races, or recovery from resource exhaustion. Safe code must not read or write
outside live, correctly typed storage even when a task fails or a channel closes.
Foreign calls are outside the safe core until their contracts are specified.

## 2. Surface sketch

```text
data Request { CountWords(text: String) }
data Reply { Count(Int) }

fn worker(jobs: Receiver<Request>, results: Sender<Reply>) {
  for (job in jobs) {
    when (job) {
      CountWords(text) -> results.send(Count(text.words().length))
    }
  }
  jobs.close()
  results.close()
}

fn main() {
  scope {
    val (jobs, incoming) = channel<Request>(capacity = 8)
    val (replies, results) = channel<Reply>(capacity = 8)
    spawn worker(incoming, replies)
    jobs.send(CountWords("one two three"))
    jobs.close()
    for (reply in results) println(reply)
    results.close()
  }
}
```

`val` binds once; mutable local storage uses `var`. `data` defines algebraic data
types. `when` is exhaustive. Functions and public type signatures are explicit;
local expression types may be inferred. Error returns use `Result<T, E>`.
Precise grammar, numeric behavior, strings, generics, and module syntax remain
open. The example illustrates the proposed rules; no compiler exists yet.

## 3. Storage and references

Each task owns its mutable program storage. Objects have stable identity during
their lifetime. Storage may be reused only after the object is dead. A reference
has a target type and an inferred root region; it cannot be dereferenced after
that region ends or while its target has a different type. No source-level
lifetime annotations are required.

Regions nest. A task's retained frames remain live while it is parked. An inner
region may end only when no live reference can reach its objects. The compiler
must reject any control-flow path that violates this rule. Exact treatment of
resizable collection shrinkage is deferred; draft 0.1 offers grow-only lists
whose elements remain stable until the owning list's region ends.

Safe code cannot construct arbitrary pointers, reinterpret object types, or
send task-local references to another task. Allocation failure stops a task
safely without exposing invalid storage.

## 4. Tasks, scopes, and channels

`spawn` starts a lightweight task. Arguments are copied if they are closed
values, or transferred if they are linear capabilities. A child cannot outlive
its enclosing `scope`; scope completion waits for all children and reports any
failure. Blocking channel operations park a task, not an OS thread.

A channel has a bounded FIFO queue. A `Sender<T>` or `Receiver<T>` handle has one
owner. `fork` creates another handle explicitly; `close` consumes a handle.
`send` copies a closed `T` into the queue. A closed type recursively contains no
task-local references, channel handles, or other task-owned capabilities.
Each queued message is delivered to exactly one receiver. A receiver reports
end after the last sender has closed and the queue is drained. Sending when
all receivers have closed reports `Closed`. Queue-full and queue-empty cases
may park the caller.

Every normal exit path must close or transfer its handles. On task failure,
the runtime closes its handles and wakes affected peers; scope observes the
failure. Failure does not run user destructors. The exact failure payload and
selection policy among multiple receivers remain open.

## 5. Middle IR contract

The first checked IR is a typed control-flow graph with block parameters and
separate *values* (computed data) and *places* (stored objects with identity).
Its operations include region begin/end, place allocation, reference creation,
load/store, typed copy, spawn, channel fork/send/receive/close, park/resume,
and scope join. Region parameters are inferred by the frontend and explicit
only in the IR. The verifier checks type, dominance, definite initialization,
region reachability, capability use, and live values across suspension.

Lowering to physical task frames, stack segments, channel layouts, and
Cranelift IR occurs after these checks. Each pass must preserve the observable
behavior of the checked IR, allowing different schedules but not new memory
errors or lost/duplicated channel messages.

## 6. Formal obligations

Let `S -> S'` be one abstract-machine transition and `WF(S)` mean all storage,
reference, channel, and capability invariants hold. The primary obligation is:

```text
WF(S) and S -> S' imply WF(S')
```

Additional obligations are: a dereference targets a live object of the right
type; mutable object ownership is unique; channel send copies only closed
values; each handle is owned or consumed exactly once; a scope cannot complete
with a live child; and failure closes task-owned handles. Progress is limited:
an enabled task can step under a fair scheduler, but arbitrary programs may
deadlock or wait forever for external input.

The event trace is an append-only *observation* of transitions for debugging
and model checking, not a required persistent production log. Deterministic
replay needs scheduler choices and external inputs in addition to IR events.

## 7. Verification plan and evidence boundary

The first Maude model checks only one task, two nested regions, one reference,
one sender, one receiver, and a one-slot queue. It explores possible orders of
region, channel, and failure operations and searches for invariant violations.
This finite result is evidence about that model only.

Next: model two tasks and `fork`, define a small-step typed IR semantics, prove
preservation for the sequential core, build a checked IR verifier and interpreter,
then compare interpreter traces with Cranelift-generated binaries. No compiler
or runtime correctness theorem is claimed at this stage.
