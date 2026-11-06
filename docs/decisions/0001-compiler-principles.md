# Decision 0001: compiler principles

Status: accepted, 2026-11-06.

Phoebe's compiler will be built around the following constraints:

1. Cranelift is the native code generator. Phoebe owns language-specific
   representation, safety checks, concurrency lowering, and cost discipline.
2. The compiler has a few documented, printable IR stages. Passes are small,
   deterministic transformations with explicit inputs, outputs, and preserved
   facts. A pass can be run in isolation on a saved IR fixture.
3. The normal compiler path runs necessary per-program type, region,
   capability, and suspension-liveness analyses. It does not run an SMT solver,
   model checker, or proof search for each user program.
4. Safety and non-pessimization claims about lowering rules are pursued as
   proof-once properties of the implemented compiler. Formal verification and
   exhaustive IR checks run when compiler code changes. A mathematical model
   alone is not evidence that the Rust implementation satisfies its theorems.
5. Costful IR operations have source-level reasons. Ordinary compilation does
   not emit a full cost ledger by default; development tools can inspect one.
6. Module interfaces and explicit dependencies support incremental builds.
   Finer-grained caching is added only when measurements justify it.
7. The compiler remains inspectable: textual IR, source spans, a pass runner,
   an IR interpreter, and reproducible cost fixtures are implementation goals.

The precise IR grammars, pass definitions, proof tool, small-aggregate threshold,
and performance targets remain open. `../compiler-architecture.md` gives the
research rationale. Accepting these principles does not claim a proof exists.
