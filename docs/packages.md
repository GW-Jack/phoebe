# Local package pins

`phoebe lock <project>` writes `phoebe.lock`; `phoebe deps <project>` verifies
that lock against the project and every local path dependency. Both commands
print the resolved package graph. For now, compilation supports only modules
listed in the root manifest. The `check`, `run`, and `build` commands still
reject `[dependencies]` while dependency name resolution is being built.

The current manifest form is:

```toml
[package]
name = "app"
version = "0.0.1"
root = "main"

[modules]
main = "main.phoebe"

[dependencies]
math = { path = "../math", version = "0.0.1" }
```

Each dependency has its own manifest, modules, and optional dependencies.
Paths are relative to the declaring manifest and versions must match exactly.
The graph is traversed in sorted dependency-alias order. A lock entry uses
its alias route, such as `math/util`, and records the package name, exact
version, and SHA-256 of the dependency manifest and all declared module
source bytes. Hash inputs are length-delimited and ordered by module name.
The root package's own source is not pinned, so editing the application does
not require a lock update. Adding, removing, or changing a dependency does.

`phoebe deps` rejects a missing or stale lock, mismatched versions, cycles,
duplicate lock entries, missing source files, and module paths that escape a
package through `..`, absolute paths, or symlinks. The current graph resolves
local path dependencies only. Registry packages, reproducible remote fetches,
dependency module imports, and a compiler cache keyed by these pins remain
open. A valid lock states which local bytes were resolved; it does not prove
that the dependency source type-checks or that a build is reproducible on a
machine without those paths.
