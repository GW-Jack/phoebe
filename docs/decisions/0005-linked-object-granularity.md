# Decision 0005: Measure linked object granularity by rebuild latency

Status: accepted, 2026-11-08.

Linked generic `build-session` keeps its exact complete-object-set reuse for
now. A native object reuse count is diagnostic evidence, not the performance
goal. The performance goal is low elapsed time from a changed source file to
an executable that reflects it, including validation, code generation,
object handling, and linking.

An uncommitted experiment exported one object per reachable concrete function
and a shared generic object. It correctly reused 121 of 122 objects after a
small edit to a generated direct call chain, but the five-run local median
edited rebuild was 93.1 ms versus 50.9 ms for re-emitting the whole object.
Cold and unchanged builds also regressed. The experiment was removed; the
figures, fixture, and limits are recorded in
`docs/performance-baseline.md`.

The non-generic in-process per-function object cache remains an experimental
path; this decision does not claim it is slower or faster on other programs.
For linked generic builds, revisit per-function or coarser native partitions
only with an end-to-end measured win on representative projects. A faster
linker, archive strategy, or a small fixed number of objects may change the
tradeoff. Keep Buck2's small compiler crates: their build parallelism and
cache units are separate from the object granularity of a compiled Phoebe
program.
