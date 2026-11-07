# Local project modules

Status: executable local modules with an in-process parsed-module session and
exact source-level interfaces. External packages, lockfiles, visibility, and
incremental Core/object rebuilds are not implemented yet.

`phoebe.toml` uses this form:

```toml
[package]
name = "example"
root = "app"

[modules]
app = "app.phoebe"
math = "math.phoebe"
```

Run `phoebe check`, `ir`, `cost`, `run`, or `build` with either the project
directory or its manifest path. The root module must define
`fn main() -> I64` for `run` without a function argument and for `build`.
The compiler currently maps that entry to the internal symbol `main`; other
functions and data types receive module-qualified names in printable Core IR.

A source file imports a listed module with `import math`. Qualified calls can
take any currently supported number and mode of arguments:

```phoebe
import math

fn relay(value: math.Maybe) -> I64 = math.unwrap(value)
fn main() -> I64 = math.add(20, relay(math.wrap()))
```

Unqualified calls, data constructors, and `when` patterns resolve within
their defining module. A qualified type name such as `math.Maybe` requires an
import. Modules are loaded in dependency order, so a data declaration may
contain a nominal type from an imported module. Import cycles and unknown
imports are rejected. The compiler maps
Core lowering diagnostics back to the defining file and local line/column.
The working example is `examples/multi_module/`.

`phoebe-project::ProjectSession` can load the same manifest repeatedly. It
compares each module's exact source bytes and path to reuse unchanged parsed
ASTs. Each successful load extracts an interface containing all data shapes
and function signatures after name qualification. Body edits leave that
interface unchanged; signature or data-layout edits change it. `LoadReport`
lists parsed and reused modules, changed interfaces, and a conservative set
of modules affected through imports. A failed load leaves the previous session
state intact. Exact structural equality, rather than a truncated hash, decides
whether an interface changed.

The current CLI starts a fresh load for each command, and Core still lowers
the combined AST on every invocation. The report records source-level
invalidation; a future per-module Core cache must additionally prove that its
stored units were checked successfully against the corresponding interfaces.
It is not a claim of incremental type checking or native code generation yet.
Cached ASTs are cloned and qualified again when a project is loaded, so the
session has no measured speed benefit.

Current limitations are explicit: imported modules cannot construct another
module's variants directly, the root module cannot be imported, imports are
module-wide rather than item-level, and all declarations in an imported
module are currently visible. Modules are compiled together on every run;
there is no cached typed interface or pinned external dependency graph. A
`[dependencies]` table is rejected so it cannot be mistaken for a resolved
package graph. A standalone `.phoebe` file still works without a manifest,
but `import` in that mode produces a diagnostic.
