# A hackable, efficient compiler — research plan

Status: principles accepted by Decision 0001, 2026-11-06. Phoebe now has an
executable compiler slice and a crate-level build graph. The cited research
informs the design; it is not evidence that Phoebe has proved any compiler
property. The current Rust verifier checks finite IR invariants during
compilation, while the Maude models remain separate bounded models.

## 1. Proof boundary

For a well-typed source program `p`, let `lower(p)` be Phoebe's checked IR and
let `B(p)` be the budget of semantic cost events justified by its operations.
The desired theorem about the core lowering is:

```text
WellTyped(p) implies
  WellFormed(lower(p))
  and Observations(lower(p)) refine Observations(p)
  and CostEvents(lower(p)) fit B(p)
```

Cost events include full-size copies, logical storage growth, dynamic bounds
checks, task creation/parking, channel operations, and indirect calls. `fit`
means no extra event in the specified classes on an execution path; it does
not mean the fewest machine instructions. An unknown index still needs a
runtime check. Proving the *rule* for eliminating a check once does not remove
the need to recognize its preconditions in each program.

Keep proof search, model checking, and exhaustive IR validation in the
compiler-development workflow. Ordinary compilation performs parsing,
type/region/capability and suspension-liveness checking, plus simple local
analyses, then applies already-vetted lowering rules. The cost ledger and full
IR dumps are available on demand. Rust types and narrow constructors make it
harder for an ordinary pass to create a cost event without a source reason;
this is engineering defense, not by itself a mathematical proof.

To claim a theorem about the *implemented* Rust lowering, the proof must be
connected to that executable code. One possible route is to verify a small,
pure lowering kernel with Verus and run that verification in CI; Verus erases
proof-only code from compiled executables. A separate Lean model that merely
resembles the Rust implementation would prove only the model. Cranelift and
linking remain outside this proposed theorem, so generated native performance
requires measurement and code inspection.

## 2. Compiler shape

```text
source / lossless syntax tree
  -> typed HIR       (names, types, regions, capability flow)
  -> Core IR         (values, places, tasks, channels, cost events)
  -> Lowered IR      (task frames, resumes, physical storage)
  -> Cranelift IR    (native code generation)
```

Each stage has one documented grammar, a parser/printer, a small interpreter
where useful, and a cheap structural verifier for development. The pass API
is a pure transformation of one module or function. Passes declare input and
output stage, preserved facts, and any cost event they are permitted to
introduce. No pass reaches into mutable global compiler state.

Keep passes small enough to inspect in isolation. Use a few stable IR stages
rather than giving each small pass a new permanent public format. A pass
runner should support `dump --before/--after`, `run-pass NAME`, and replay of
textual IR fixtures. Stable source spans and entity IDs connect diagnostics
and cost events across stages. These tools are part of hackability, not
optional polish.

For transformations that fit local rewrite rules, a small rule language plus
a once-verified execution engine is attractive: adding a rule entails proving
or checking the rule during compiler development, while ordinary compilation
only matches and applies it. A transformation that does not fit this engine
needs its own correctness argument. This preserves room for experimentation
without treating every new pass as automatically verified.

Module interfaces and dependency hashes determine what to recompile after
an edit. Begin with a simple explicit dependency graph and per-module cache;
add finer-grained demand-driven queries only if measured rebuilds need them.
The normal path should not serialize and reparse IR between in-process passes.

## 3. Research that shapes this plan

- [CompCert](https://www.cs.cmu.edu/~15811/papers/compcert.pdf) shows the
  proof-once approach: prove compiler passes preserve program behavior, then
  apply them to arbitrary programs without proving each compilation anew.
  Phoebe's cost claim needs cost-aware semantics in addition to behavior.
- [XCert](https://www.cs.cornell.edu/~lerner/papers/pldi10-xcert.pdf) combines
  checked optimization rules with a verified execution engine. It motivates
  a narrow extension point for safe, hackable rewrites.
- [Nanopass for Chez Scheme](https://www.cs.tufts.edu/comp/150FP/archive/icfp13.pdf)
  demonstrates that small single-purpose passes can support a real compiler.
  Its measured compile times were roughly 1.6–1.75 times the earlier compiler
  in the reported setup, so pass count and repeated traversals still matter.
- [MLIR's design paper](https://arxiv.org/pdf/2002.11054) argues for retaining
  high-level semantics until the passes needing them have run, and for
  parser/printer, locations, and validation as shared IR infrastructure.
  Phoebe can adopt those principles with a smaller, language-specific IR.
- [Adapton](https://matthewhammer.org/adapton/adapton-pldi2014.pdf) formalizes
  demand-driven incremental recomputation. It motivates explicit dependency
  tracking for fast rebuilds, without requiring that whole framework in v1.
- [Alive2](https://web.ist.utl.pt/nuno.lopes/pubs.php?id=alive2-pldi21)
  illustrates the complementary route of validating individual compiler
  translations. Its bounded checks are valuable for development and CI but
  would add the per-compilation solver work Phoebe wants to avoid.
- [Verus](https://www.wisdom.weizmann.ac.il/~padon/verus-sosp2024.pdf) offers
  a possible way to verify Rust implementation code offline while erasing
  proof-only code from the executable. Its applicability to Phoebe's exact
  lowering remains to be tested.

## 4. First implementation slice

Implement only `identity`, same-task borrow, in-place list return, and
checked indexing before adding concurrency lowering. The slice needs a
textual Core IR, a reference interpreter, the Cranelift path, source spans,
and the cost-fixture runner. Prove or verify the simple lowering rules, then
add `spawn` and channel send with the same boundary. This will test whether
the architecture is genuinely easy to edit while keeping ordinary builds
fast enough to measure.
