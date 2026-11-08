# Phoebe language specification — draft 0.5

Status: design proposal, 2026-11-06. Syntax examples are illustrative. The
current compiler subset and isolated runtime kernel are described in
`version-0.0.1.md`; the bounded machines in `formal/` cover different
abstract states.

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

`val` fixes a binding; `var` permits rebinding. Either may own mutable local
storage, whose mutation requires exclusive access. `data` defines nominal
product and sum types; `when` is exhaustive. Function and public type
signatures are explicit; local expression types may be inferred. `Option<T>`
represents absence and `Result<T, E>` represents expected failure. The type,
trait, operator, error, and package baseline is in `docs/language-features.md`
and Decision 0003. Precise grammar and several ABI details remain open. The
example illustrates proposed rules; the compiler does not parse this
concurrency surface yet.

## 3. Storage and references

Each task owns its mutable program storage. Objects have stable identity during
their lifetime. Storage may be reused only after the object is dead. A reference
has a target type and an inferred root region; it cannot be dereferenced after
that region ends or while its target has a different type. No source-level
lifetime annotations are required.

Regions nest. A task's retained frames remain live while it is parked. An inner
region may end only when no live reference can reach its objects. The compiler
must reject any control-flow path that violates this rule. Exact treatment of
resizable collection shrinkage is deferred; draft 0.5 offers grow-only lists
whose elements remain stable until the owning list's region ends.

Safe code cannot construct arbitrary pointers, reinterpret object types, or
send task-local references to another task. Allocation failure stops a task
safely without exposing invalid storage.

Function parameters use `T` for ownership, `&T` for a shared borrow, and
`&mut T` for an exclusive borrow. Small `Copy` values copy by value; a larger
value moves into an owning parameter. An independent large copy is explicit
as `copy(x)`. A `val` owner may mutate its object through an exclusive borrow
without rebinding its name. Borrow roots and last uses are inferred, but
borrows cannot enter a child task or message.

## 4. Tasks, scopes, and channels

`spawn` starts a lightweight task. Scalars and small fixed-size closed
arguments copy by value. A variable-size or large fixed-size closed
argument requires explicit `copy(x)` at the call site; linear capabilities
transfer ownership. The threshold for a small fixed-size aggregate remains
open and will be specified independently of backend accident. A child cannot
outlive its enclosing `scope`; scope completion waits for all children and
reports any failure. Blocking channel operations park a task, not an OS thread.

A channel has a bounded FIFO queue. A `Sender<T>` or `Receiver<T>` handle has one
owner. `fork` creates another handle explicitly while the original remains
usable; `close` consumes a handle. A closed handle cannot be forked again.
`send` accepts a sendable `T`. A sendable type recursively contains only closed
values and explicitly transferable, linear communication capabilities. Closed
values contain no task-local references or task-owned capabilities and are
copied into the queue. A linear capability moves into the queue at successful
enqueue, then to the receiving task at dequeue. It cannot be copied or used by
the sender after that move. Arbitrary references and mutable task storage are
never sendable. If all receivers have closed, `send` returns `Closed(message)`;
the caller retains ownership of every capability in the returned message.
For a full queue, a parked sender retains ownership until enqueue commits.
Each queued message is delivered to exactly one receiver. A receiver reports
end after the last sender has closed and the queue is drained; an absent forked
handle does not count as an open sender. Sending when all receivers have closed
reports `Closed(message)` without queueing it. Closing the last receiver may
discard queued messages. The runtime closes any capabilities owned by a
discarded message exactly once; this disposition is explicit in the formal
model. A receiver that has dequeued a capability must close or transfer it
before normal exit; failure closes its owned capabilities.
Queue-full and queue-empty cases may park the caller.

`reply<T>()` creates a one-shot `ReplyTo<T>` and `ReplyReceiver<T>` in the
current scope. `ReplyTo<T>` is a linear communication capability and may move
inside a request message. `complete(value)` consumes it without parking and
returns `Delivered` or `Closed` if the receiver was canceled. A reply receiver
can receive one value or be canceled; canceling it discards a queued reply.
`ReplyTo<T>` cannot escape the creating scope. This is the initial concrete
capability-transfer case; general transferable handle types still need their
own semantics and proofs.

The implemented `I64` surface binds both rights with
`val (right, receiver) = reply<I64>()?`. `right.complete(value)` returns
`Complete<I64>.Delivered` or `Complete<I64>.Closed(value)` and consumes the
right on either path. `receiver.receive()` returns `Reply<I64>.Value(value)`
or `Reply<I64>.End` and consumes the receiver. Either right can be explicitly
closed; a `with` block closes an unspent right on exit. A `ReplyTo<I64>` may
move to a spawned child. The concrete `I64` request surface now packages a
reply right, sends and receives it through structural `when` arms, and
unpacks or closes it. General capability-bearing payloads remain open.

Scoped tasks and channels are the concurrency core. An actor is a library or
surface abstraction: one scoped task owns its state, reads a mailbox, and runs
one handler at a time. Its mailbox obeys the same bounded channel and linear
capability rules. An actor handler must not park on a full mailbox; its send
policy must be explicit. Actor syntax and long-lived state storage are open.

Every normal exit path must close or transfer its handles. On task failure,
the runtime closes its handles and wakes affected peers; scope observes the
failure. Failure does not run user destructors. The exact failure payload and
selection policy among multiple receivers remain open.

`?` propagates an existing `Err` as a normal early return; it performs no
implicit `Option` or error-type conversion. A lexical `with` owns a built-in
communication capability or an aggregate containing them and closes any
still-owned capabilities on every normal exit, including `?`. A rejected send
returns its intact message. The caller may recover it or consume it with the
compiler-derived, structural `close_caps()`; this runs no user code. A named
`send_or_close` convenience operation may explicitly discard it on rejection.
Decision 0004 records the ownership and cleanup rule.

## 5. Modules, generics, and operators

Modules use explicit imports and exports, private declarations by default,
and no executable import-time initialization. Packages have a manifest and
pinned lockfile. A canonical interface fingerprint controls downstream
recompilation but does not promise semantic compatibility or native ABI
stability. Generic functions compile once per compatible ABI shape, with
dictionaries for trait operations unavailable statically; concrete functions
use direct calls. A later runtime-optimized mode may specialize measured hot
paths. Only fixed operator symbols may map to named traits, without implicit
conversions or hidden parking. See `docs/generic-core.md` for the parametric
Core contract and `docs/language-features.md` for the broader language sketch.

## 6. Middle IR contract

The first checked IR is a typed control-flow graph with block parameters and
separate *values* (computed data) and *places* (stored objects with identity).
Its operations include region begin/end, place allocation, shared and
exclusive borrow, load/store, typed move/copy, lexical close, error return,
spawn, channel fork/send/receive/close, park/resume, and scope join, plus
capability moves into and out of messages. Region parameters are inferred by
the frontend and explicit only in the IR. The frontend and lowering must check
type, definite initialization, region reachability, borrow exclusivity,
capability use, message ownership at enqueue/dequeue/discard, structural
cleanup on all
normal exit edges, and values live across suspension for each program. A full
IR verifier rechecks dominance and these
invariants during compiler development and CI; release builds may omit that
redundant pass when IR is created by trusted, proved lowering operations.

Lowering to physical task frames, stack segments, channel layouts, and
Cranelift IR occurs after these checks. Each pass must preserve the observable
behavior of the checked IR, allowing different schedules but not new memory
errors or unintended loss/duplication of channel messages or capabilities. The
checked IR also carries cost events and provenance under the non-pessimization contract in
`docs/non-pessimization.md`. This contract constrains Phoebe's lowering; it does
not assert a minimum machine-instruction count from Cranelift. The intended
proof-once architecture is described in `docs/compiler-architecture.md`.

## 7. Formal obligations

Let `S -> S'` be one abstract-machine transition and `WF(S)` mean all storage,
reference, channel, and capability invariants hold. The primary obligation is:

```text
WF(S) and S -> S' imply WF(S')
```

Additional obligations are: a dereference targets a live object of the right
type; mutable object ownership is unique; channel send copies only closed
values or explicitly moved linear communication capabilities; each capability
is owned or consumed exactly once, including while queued or discarded; a
scope cannot complete with a live child or owned reply capability; and failure
closes task-owned capabilities. Progress is limited:
an enabled task can step under a fair scheduler, but arbitrary programs may
deadlock or wait forever for external input.

The event trace is an append-only *observation* of transitions for debugging
and model checking, not a required persistent production log. Deterministic
replay needs scheduler choices and external inputs in addition to IR events.

## 8. Verification plan and evidence boundary

The first Maude model checks one task, two nested regions, one reference, one
sender, one receiver, and a one-slot queue. The second checks two tasks,
explicit handle `fork`, transfer at spawn, distinct messages, close, failure,
and scope outcome. The third checks a one-shot `ReplyTo` moving through a
request queue, its completion and cancellation, queued-message discard, child
failure, and scope outcome. They explore possible operation orders and search for
encoded invariant violations. Their precise bounds are in `formal/README.md`.

Next: combine regions and task transfer in a typed small-step IR semantics,
prove preservation for its sequential core, build a checked IR verifier and
interpreter, then compare interpreter traces with Cranelift-generated binaries.
No compiler or runtime correctness theorem is claimed at this stage. The
non-pessimization examples are proposed compiler acceptance criteria, not
tests that can run before an IR implementation exists.
