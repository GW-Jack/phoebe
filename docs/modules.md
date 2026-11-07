# Local project modules

Status: executable local modules with in-process parsed-module and checked-Core
function reuse sessions. Local path dependency lock verification is available
through `phoebe lock` and `phoebe deps`; dependency compilation, visibility,
and incremental native object rebuilds are not implemented yet.

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

`phoebe::ProjectCompilerSession` extends this with in-process checked-Core
function-body reuse. The compiler compares the old and new function ASTs,
allowing a uniform source-span shift. It also requires every nominal data
shape and function signature to retain its position and ABI. If the global
layout changes, it lowers all functions afresh. A body edit recompiles that
function while unchanged functions can reuse verified bodies, even within
the edited module. Core reruns its program-wide verifiers before publishing the new session state;
failed compilation preserves the old state. `CompileReport` counts reused
functions. Cached spans are rebased to the new combined source positions.

The current CLI starts a fresh session for each command. This API does not
provide incremental type checking, persistent caches, or native object reuse.
Cached ASTs are cloned and qualified again when a project is loaded. The
small warm-session baseline in `docs/performance-baseline.md` shows only a
marginal difference on a six-function project.

Current limitations are explicit: imported modules cannot construct another
module's variants directly, the root module cannot be imported, imports are
module-wide rather than item-level, and all declarations in an imported
module are currently visible. There is no cached typed interface or pinned
external dependency graph in the compiler. A `[dependencies]` table is
rejected during compilation; `docs/packages.md` describes the separate
lockfile graph commands. A standalone `.phoebe` file still works without a manifest,
but `import` in that mode produces a diagnostic.
