# Collection representation experiment — draft 0.1

Status: exploratory design and isolated Rust benchmark, 2026-11-08. No Phoebe
collection syntax, collection IR operation, representation selector, or
collection runtime is implemented. This document does not amend Decision 0003.

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

The normal compiler needs only deterministic eligibility checks and a small
cost heuristic over a bounded candidate table. A separate development or CI
workflow can check each candidate's refinement, reference-stability, failure,
and effect claims. Such a proof must eventually connect to the implementation;
a Maude model alone would establish only a model property. `phoebe explain`
should report candidate exclusions, the chosen layout, and the cost evidence.
No choice is called "best" without a stated workload and objective.

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
