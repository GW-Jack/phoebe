# Collection representation experiment — draft 0.3

Status: exploratory design, isolated Rust benchmark, pure requirement
planner, host-side scalar storage kernel, and a bounded fixed-array Core
slice, 2026-11-08.
`crates/collection-plan` infers requirements from a supplied checked-use graph
and selects by constant-size rule logic; it is not yet fed by Phoebe source or
Core IR. `crates/collection-storage` exercises selected fixed and contiguous
layouts, but is not yet Phoebe's interpreter or native runtime.
Checked Core supports inline `[I64; N]` for `N <= 32`, literal indexed reads,
and length. Source accepts local literals such as `[40, 2]`, `array[0]`, and
`array.len`. Both `val` and `var` element reads select one component; the
direct and retained Cranelift paths and interpreter execute the same program.
Array types in source signatures, dynamic indexing, element mutation, larger
fixed storage, and source-use inference remain open.
This document does not amend Decision 0003.

## The semantic boundary

An abstract ordered sequence can leave layout unspecified, but cannot leave
observable behavior unspecified. The first common contract is:

- `push_back(value)` appends one owned value or returns the intact value on a
  resource failure. No element becomes visible on failure.
- `len`, `get`, and iteration observe insertion order. `get` checks bounds.
- `with element = &sequence[index] { ... }` borrows an existing element. A
  structural mutation that could move or remove that element is rejected while
  the loan is live.
- Moving the sequence transfers all its elements and storage authority;
  copying it requires an explicit `copy(sequence)` and an independent store.
- Sending it to another task transfers the whole closed sequence by ownership.
  Borrowed element references never enter a message.

These operations form a proposed `Sequence<T>` family. They do not promise
that an element loan survives growth. A separate `StableSequence<T>` family
would promise that `push_back` preserves the address and identity of every
existing element, so an element loan may span append. Both families must still
reject shrinking, inserting before, or reusing a live borrowed element. The
compiler must check element loans against *logical identity*, not only raw
address: shifting elements in place can change what an old address denotes.

Illustrative source, not accepted by today's parser:

```text
val ordinary: Sequence<Job> = sequence()
ordinary.push_back(first_job)?
with first = &ordinary[0] {
  use(first)
  // ordinary.push_back(job) cannot run under this loan.
}
ordinary.push_back(job)?

val stable: StableSequence<Job> = stable_sequence()
stable.push_back(first_job)?
with first = &stable[0] {
  stable.push_back(job)?   // allowed only for a certified stable layout
  use(first)
}

val exact: SegmentedSequence<Job> = SegmentedSequence()
```

The abstract constructor chooses a concrete representation at its construction
site. An explicit concrete name pins it. An inferred choice is static, with no
per-operation dynamic dispatch. Public, separately compiled APIs must either
fix a representation, expose an opaque implementation with a stable ABI, or be
generic over a representation contract; local choice cannot silently change a
published layout or force downstream recompilation from private uses.

## Infer requirements before estimating speed

The default selector should first infer the **least collection capability**
that satisfies all observed uses. It is not tasked with finding a globally
fastest layout. Avoiding unnecessary growth machinery, relocation, node links,
and dynamic dispatch is a correctness-checkable baseline; comparing two
eligible layouts on real hardware is a separate performance question.

Here, collection capability means an operation or guarantee such as fixed
extent, append, removal, indexed access, or stable element identity. It does
not mean a transferable communication right. The semantic analysis should
infer these capabilities from reachable operations and loan lifetimes, not
from naming or superficial syntax. A collection initialized by repeated
appends and never changed afterward can have a build phase followed by a
fixed phase. A collection whose length is known at creation and never changed
needs no growth path. A collection that is only appended to needs no removal
or shrink path. None of these conclusions follows from a single example path;
the analysis must account for every reachable branch and summarized call.

Let `Req(s)` be the operations and guarantees required at construction site
`s`, and `Cap(R)` those provided by representation `R`. The eligibility
condition is `Req(s) subseteq Cap(R)`, together with ownership, loan, failure,
and ABI compatibility. The normal compiler then applies a deterministic,
bounded rule among eligible layouts. Its soundness obligation is to
over-approximate actual uses in `Req(s)` and preserve the abstract sequence
contract after lowering. The default's performance claim is narrower:
lowering must not add growth, relocation, bounds checks, or indirect calls
whose required capability and source operation are absent. This claim can be
checked against IR effects without proving that the selected eligible layout
is fastest. Empirical benchmarks may later revise the default ordering.

For each construction site, the typed IR records a finite requirement summary:

| Axis | Facts to infer from uses |
| --- | --- |
| Extent | Compile-time length; run-time length fixed at creation; build then freeze; append-only; arbitrary structural mutation. |
| Access | Whole scan; front/back; indexed lookup; stable element handle; splice or keyed lookup. |
| Loans | No interior loan; loan ends before mutation; loan spans append; loan spans suspension. |
| Ownership | Closed values; affine or capability-bearing elements; copy requested; transfer to task or channel. |
| Escape | Local construction and consumption; call to a summarized function; public or unknown boundary. |

Facts combine by a monotone join over control-flow paths and call summaries.
For example, one path that removes an element prevents an append-only claim;
one loan spanning append requires stable append even if every other loan ends
earlier. An unknown call must use its declared effects or a conservative
summary. The finite lattice permits a local worklist fixed point without a
solver in normal compilation. Summaries belong in module interfaces so a
private body edit that preserves them need not invalidate callers. At a public
ABI boundary the representation is fixed or generic, not inferred from
unseen downstream programs.

The default rules to test are deliberately simple:

| Inferred use | Default representation direction | Unneeded machinery |
| --- | --- | --- |
| Compile-time fixed `N`, no structural mutation | Inline `[T; N]` when frame size permits; otherwise fixed region storage | Capacity field, growth path, per-element allocation. |
| Run-time size known once, then fixed | One exact-size allocation, constructed in place | Geometric growth and relocation. |
| Build by append, then freeze | A construction buffer with a phase transition to a fixed view; retain its allocation unless an explicit compaction pays to move it | Growth checks after freeze, implicit final copy. |
| Append-only, with no element loan across append | Contiguous growth by default | Node links, chunk directory, stable-address promise. |
| Append with an element loan still live | A certified stable layout, initially segmented storage | Any relocation of a borrowed element. |
| Removal, insertion, or splicing by stable handle | An indexed region or linked layout if the handle contract requires it | O(n) search for an already known node, when avoidable. |

These are defaults, not a commitment that every fixed array belongs on a task
stack. A large fixed value in a retained task frame may need region storage.
Likewise, a sequence whose length stops changing after construction still
needs a length field if indexing must check a run-time bound. If a program
uses only append and scan, a linked list should not become the default merely
because it implements the abstract operations. A programmer can still request
a concrete layout.

The selector should emit a small explanation, for example:

```text
site 41: append-only; indexed reads; no loan spans append
eligible: contiguous, segmented, region-linked
chosen: contiguous (no stable-address requirement; direct indexed access)
excluded: fixed array (length changes after construction)
```

This explanation is an inspectable planning result. The normal build does not
benchmark alternatives or prove them anew. A profile-guided build may replace
the default among semantically eligible representations, with its measured
workload and objective recorded separately.

The first executable kernel is in `crates/collection-plan`. `summarize_path`
folds one checked use sequence; `summarize_graph` runs a finite forward
worklist over already-checked collection uses. It tracks building and frozen
states separately at joins and loops, so one frozen path makes a later append
invalid even if another path remains in the build phase. It ignores
unreachable uses and grants a freeze guarantee only when every reachable exit
is frozen. Calls are explicit uses: a checked callee contributes its
collection-specific effect summary, and an unknown call is rejected rather
than treated as pure. Those summaries still have to be derived from Phoebe
functions and stored at module boundaries.
`Requirements::join` conservatively merges use facts; `select` applies a
bounded rule table and returns possible reserve, grow, relocate, shift, and
chunk-allocation effects. Fixed-size selections have no growth effect, and
removal-only selections do not acquire one. A stable element loan spanning
append excludes the moving contiguous layout. The kernel explicitly rejects
a stable-handle plus indexed-access mix because none of its current candidates
supports both without an additional index. It also rejects a layout choice
escaping an unknown ABI boundary. Tests cover these cases in Cargo and Buck2.
The upstream source/Core use extraction, loan analysis, call summaries, and
representation implementations are still required; these tests do not prove
their claims.

The first storage kernel accepts a checked `Plan` with extent and element size
attached. `FixedInlineI64<N>` stores exactly `N` scalar words with no heap
allocation or growth method. `FixedRegionI64` allocates once before copying
its initial values and exposes no growth method. `AppendI64` uses fallible
reserve before each push, returns the intact scalar on allocation failure,
and consumes itself into `FrozenI64` without moving its elements. A bounded
push rejects at the element limit before attempting reserve or changing the
sequence. Fixed-region construction checks the same limit before allocation.
These are distinct Rust types, so ordinary reads do not dispatch on a representation
tag. The host `FixedRegionI64` retains Rust `Vec` metadata; an eventual native
fixed layout should carry only the extent and storage required by its ABI.
Segmented and linked representations, task-local region integration, general
element ownership, and source-to-Core lowering remain open.

The first pass obligations are mechanical: inferred requirements must be a
sound over-approximation of every reachable use; the selected layout must
satisfy those requirements; and lowering must introduce only effects allowed
by that layout and source operations. In particular, a fixed-size selection
must emit no `storage.grow` or `move.elements` site after construction, an
append-only summary must emit no shrink operation, and a no-index program
must not acquire an indexed bounds check. These are useful proof targets even
when the default layout choice itself is a heuristic.

## Candidate layouts

| Layout | `push_back` | Indexed `get` | Element address across growth | Storage and caveat |
| --- | --- | --- | --- | --- |
| Contiguous growable buffer | Amortized O(1); occasional relocation | O(1) | Not guaranteed | One buffer; growth may allocate and move existing values. |
| Segmented buffer, fixed-size chunks | Amortized O(1) | O(1), quotient and remainder | Stable if chunks never move or reuse live slots | One allocation per chunk plus a chunk directory; iteration crosses chunks. |
| Region-backed linked nodes | O(1) with a tail handle | O(n) | Stable if the region never moves or reuses live nodes | Node links and a region reservation; removing by a known handle can be O(1), but lookup cannot. |

For linked nodes, "region-backed" describes a proposed Phoebe allocation
strategy, not the benchmark's Rust allocator. A growable region needs an
implementation that preserves node addresses. Pre-reserving a maximum, using
nonmoving slabs, and using indices instead of references have different
limits and cost effects. The benchmark below uses a known maximum and one
preallocated Rust `Vec<Node>`; it tests traversal/layout cost, not an
unbounded region allocator or Phoebe's borrow checker.

The common `Sequence<T>` benchmark uses `push`, `get`, and ordered scan.
It does not test removal, splicing, element borrowing, allocation failure,
concurrency, generic `T`, or task suspension. Linked-node O(1) removal would
require a handle operation and a different contract. This is why one abstract
type with every possible collection method would be misleading.

## Reproducible host-side probe

`examples/bench_sequence_representations.rs` implements the same narrow
operation set three times. The contiguous buffer and linked node arena reserve
the known maximum once; the segmented buffer reserves a chunk directory and
allocates 256-element chunks. A build-and-scan measurement includes
construction and a full ordered sum. Indexed reads use deterministic,
spread-out indices. Every round checks the exact sum and read checksum.

```sh
rustc --edition=2021 -O examples/bench_sequence_representations.rs \
  -o /tmp/phoebe-sequences
/tmp/phoebe-sequences 100000 1000 9
```

One local macOS ARM64 run on 2026-11-08 produced these medians across nine
rounds. Values are elapsed wall time for this executable, not Phoebe native
code, and are too small and host-dependent to rank layouts generally.

| Layout | Build plus scan, 100,000 elements | 1,000 indexed reads |
| --- | ---: | ---: |
| Contiguous | 0.117 ms | 0.001 ms |
| Segmented | 0.158 ms | 0.001 ms |
| Region-linked | 0.258 ms | 86.703 ms |

The result exposes one decisive constraint: a linked layout with no auxiliary
index is a poor choice for an indexed-read workload. It does not establish
that contiguous storage always wins, nor does it measure the stable-reference
or handle-removal workloads for which other layouts may qualify.

## Proposed checked IR facts and effects

The selector should see semantic requirements before choosing a layout:

```text
%s = sequence.new<T> {family=Sequence, site=41, region=r7}
%v = sequence.push %s, %job {failure=return_owned, site=42}
%e = borrow.element %s, %index {loan=l9, identity=element}
borrow.end l9
%x = sequence.get %s, %index {check.bounds, site=43}
// inferred: append_only, indexed, no_loan_across_append
// selected: contiguous; growth may move elements only with no live loan
```

After selection, `sequence.new` records a concrete representation ID and its
capabilities. The operation's effect plan records *possible* cost events,
not a claim that every execution allocates:

| Effect or fact | Meaning | Required source reason |
| --- | --- | --- |
| `storage.reserve` | Initial buffer, chunk directory, or node region | Collection construction or explicit reserve. |
| `storage.grow` | New capacity or committed region storage | `push` or `reserve`. |
| `move.elements` | Relocation of existing values | Contiguous growth; forbidden with a live element loan. |
| `copy.bytes` | Independent duplicate of variable-size values | Explicit `copy(sequence)` or another named value-semantic boundary. |
| `alloc.chunk` | New stable chunk or slab | Growth crossing its current capacity. |
| `check.bounds` | Dynamic index validation | `get` or element borrow without a local range proof. |
| `loan.element` | Root and identity of a borrowed element | Explicit element borrow. |
| `may_relocate`, `stable_append` | Representation capability | Certified layout description. |

`copy.bytes` should not be used to describe an affine move of elements. A
relocation of closed values has its own `move.elements` effect; moving a value
containing communication rights needs an exact transfer plan. A selector may
choose a moving representation only if its required operations never allow
relocation under a live loan. A `StableSequence` candidate must establish
`stable_append` and a compatible element identity rule. All allocation and
copy/move effects remain visible in Phoebe's cost ledger; selection must not
launder them behind an abstract call.

The intended safety condition for every selected representation `R` is a
simulation relation `abstract_R(concrete_state) = logical_sequence` that is
preserved by each successful operation. If `loan(element, address)` is live,
the same logical element remains initialized at that address until the loan
ends. Failure transitions leave the abstract sequence and incoming owner
unchanged. Because layouts can fail allocation at different pushes, the
abstract contract permits resource failure at each fallible growth operation;
it does **not** promise identical failure timing across representations.

The normal compiler needs deterministic eligibility checks and a bounded,
documented default rule over a small candidate table. A separate development or CI
workflow can check each candidate's refinement, reference-stability, failure,
and effect claims. Such a proof must eventually connect to the implementation;
a Maude model alone would establish only a model property. `phoebe explain`
should report candidate exclusions, the chosen layout, and the inferred facts.
No choice is called "fastest" without a stated workload and objective.

## Research inputs

- [Goose's array specification](https://aardappel.github.io/goose/docs/goose_spec.html)
  separates grow-only, grow-shrink, limited, and fixed arrays. Its stable
  growth relies on each live resizable array topping one of several reserved
  data stacks. Phoebe has not adopted that storage rule; many suspended tasks
  and live collections make its address-space cost a question to measure.
- [Cozy](https://homes.cs.washington.edu/~mernst/pubs/generalized-synthesis-icse2018.pdf)
  synthesizes data structures from abstract state, queries, and updates. Its
  search can use hours; Phoebe's ordinary compiler needs a bounded selector.
- [Fiat](https://www.cs.purdue.edu/homes/bendy/papers/Fiat/index.html)
  uses proof-carrying refinement of abstract data types. Phoebe can borrow
  the refinement boundary without claiming its Rust implementation is proved.
- [CHAMELEON](https://www.sri.inf.ethz.ch/publications/shacham2009chameleon)
  uses runtime collection metrics to propose adaptations. That motivates an
  optional profile-guided mode, not changing semantics or inserting runtime
  adaptation into every collection.

## Next bounded experiment

Build an executable Core-only `Sequence<I64>` slice with `new`, `push`,
`get`, and `borrow.element`; add two layouts first. Check the same source in
the interpreter and native backend, then model stable append and forbidden
shrink under a live element loan. Add linked nodes only when a handle-based
operation makes their advantage testable. Keep the selector out of hot normal
compilation until the layouts and effect accounting are sound.
