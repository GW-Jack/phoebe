# Local package pins

`phoebe lock <project>` writes `phoebe.lock`; `phoebe deps <project>` verifies
that lock against the project and every local path dependency. Both commands
print the resolved package graph. `check`, `run`, and `build` verify the lock
before compiling root and dependency modules together.

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
Compilation parses the same source-byte snapshots used for lock verification.
The root package's own source is not pinned, so editing the application does
not require a lock update. Adding, removing, or changing a dependency does.

Each package may import its own modules and the root module of each direct
dependency. In a source file, `import math` makes `math.answer(20)` available;
the alias `math` names that dependency's root module. A dependency may in turn
import one of its own dependencies. Internal symbols carry the complete alias
route, such as `math__util.add`, so unrelated packages cannot collide. The
working example is `examples/package_graph/app/`; it runs as 42 through the
interpreter, Cranelift JIT, and standalone executable.

`phoebe deps` rejects a missing or stale lock, mismatched versions, cycles,
duplicate lock entries, missing source files, and module paths that escape a
package through `..`, absolute paths, or symlinks. The current graph resolves
local path dependencies only. Registry packages, reproducible remote fetches,
item-level visibility, direct imports of a dependency's non-root modules, and
a compiler cache keyed by these pins remain open. A valid lock states which
local bytes were resolved; it does not prove that the dependency source
type-checks or that a build is reproducible on a machine without those paths.
