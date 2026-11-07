# Local project modules

Status: executable first slice. A project manifest groups local `.phoebe`
files and gives one root module. External packages, lockfiles, visibility,
module interfaces, and incremental rebuilds are not implemented yet.

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
import. Import cycles and unknown imports are rejected. The compiler maps
Core lowering diagnostics back to the defining file and local line/column.
The working example is `examples/multi_module/`.

Current limitations are explicit: imported modules cannot construct another
module's variants directly, the root module cannot be imported, imports are
module-wide rather than item-level, and all declarations in an imported
module are currently visible. Modules are compiled together on every run;
there is no interface cache or pinned external dependency graph. A
`[dependencies]` table is rejected so it cannot be mistaken for a resolved
package graph. A standalone `.phoebe` file still works without a manifest,
but `import` in that mode produces a diagnostic.
