# Local project modules

Status: executable local modules with in-process parsed-module and checked-Core
function reuse sessions. Content-pinned local path dependencies compile into
the same program. Declarations are private by default and `pub` exports a
function or data type. An in-process native build session can reuse direct
function objects; the ordinary CLI build uses one-shot emission.

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
import. Public imported nominal variants may be constructed as
`math.Maybe.None` or `math.Maybe.Some(22)` and matched with qualified `when`
arms. Modules are loaded in dependency order, so a data declaration may
contain a nominal type from an imported module. Import cycles and unknown
imports are rejected. A qualified reference to a private function or type
gets a source-located diagnostic. The compiler maps
Core lowering diagnostics back to the defining file and local line/column.
Module ordering uses an explicit DFS stack, so a long import chain does not
consume the compiler's call stack. A bounded 8,192-module graph test checks
ordering and cycle rejection; it is not a measured project-size limit.
The local module example is `examples/multi_module/`. A package may import a
direct dependency's root module by its alias; see `docs/packages.md` and
`examples/package_graph/app/`.

`phoebe-project::ProjectSession` can load the same manifest repeatedly. It
compares each module's exact source bytes and path to reuse unchanged parsed
ASTs. Each successful load extracts an interface containing exported function
signatures (parameter modes and types, excluding local parameter names),
exported data shapes, and any private data shapes reachable through those
public signatures. A private type may remain opaque to importers while
its layout still invalidates their build when it changes. Body edits leave
that interface unchanged; relevant signature or data-layout edits change it. `LoadReport`
lists parsed and reused modules, changed interfaces, and a conservative set
of modules affected through imports. A failed load leaves the previous session
state intact. Exact structural equality, rather than a truncated hash, decides
whether an interface changed.
The session constructs reverse import edges once per load and walks them from
changed interfaces. A body-only edit affects its defining module without
invalidating importers.
`ModuleInterface::canonical_bytes()` now encodes the ordered source-visible
facts with length prefixes and a format version; `fingerprint()` returns its
SHA-256 digest. Body text, local parameter names, and source spans are absent.
Reachable private data layouts are included. The in-process session still
uses exact structural equality, so a digest collision cannot make it reuse a
stale interface. The digest is a candidate key for a future persistent cache,
not a deployed cache format or a stable external ABI.

`phoebe::ProjectCompilerSession` extends this with in-process checked-Core
function-body reuse. The compiler compares the old and new function ASTs,
allowing a uniform source-span shift. Referenced nominal data layouts must
retain their shapes and existing function signatures must retain their ABI.
Unrelated data declarations may move nominal IDs: cached types, constructors,
and product operations are remapped by name. Nested payload and field layouts
are checked transitively. Functions
may move to new indices when another function is inserted or removed: reused
Core calls and spawn targets are remapped by function name. A nominal layout
change in an existing signature still lowers all functions afresh. A body edit recompiles that
function while unchanged functions can reuse verified bodies, even within
the edited module. Core reruns its program-wide verifiers before publishing the new session state;
failed compilation preserves the old state. `CompileReport` counts reused
functions. Cached spans are rebased to the new combined source positions.
Adding a private helper therefore need not re-lower existing checked bodies;
native object reuse remains governed by its separate codegen keys. The
in-process native session assigns stable IDs to nominal names and retains
direct objects across unrelated data insertions. It conservatively clears
those objects if a previously seen data layout changes or disappears.
Core uses one traversal for every instruction and terminator source span,
including `spawn` and `scope.join`. The native direct-object cache removes
those spans from its function key, so moving unchanged source text does not
force code emission solely because locations changed.
`NativeBuildSession` now keys direct objects by qualified function name and
normalizes call and spawn targets to those names. Internal linker symbols use
an injective encoding of the name, so inserting a private helper does not
rename existing objects. The session compares existing signatures, runtime
context needs, suspension flags, and frame facts before reuse. A changed
retained child frame invalidates direct parents that could spawn it. Poll
objects are still emitted afresh, and these symbols are not a public ABI.

The current CLI starts a fresh parse and Core session for each command.
`phoebe build-session <project> -o <output>` exposes the in-process compiler
and native sessions through a line-oriented interface: send `build` after an
edit, inspect the reuse counts, and send `quit` when finished. A body-only edit
with a stable public interface can preserve checked Core and direct native
objects for unrelated functions. The session itself holds these caches in
memory; restarting it begins with a cold build.
`phoebe build <project> -o <output> --cache-dir <directory>` can persist
the complete emitted object set across compiler invocations when its codegen
inputs are unchanged. The cache key excludes source spans and the bodies of
unreachable pure leaf functions; it retains their signatures and all other
checked Core. Every hit still verifies the full Core program. The cache is
versioned and bound to the local compiler
executable's path, size, modification time, and file identity. It checks an
exact codegen key and discards truncated or corrupt entries. The linker runs on
every build. The cache is optional and does not persist typed interfaces or
incremental type checking. A changed Core program recompiles its object set;
in-process `NativeBuildSession` still reuses direct function objects after
individual body edits or harmless function-index shifts.
Cached ASTs are cloned and qualified again when an in-process project session
is loaded. The small warm-session baseline in `docs/performance-baseline.md` shows only a
marginal difference on a six-function project.

Current limitations are explicit: the root module cannot be imported, and imports
are module-wide rather than item-level. There is no cached typed interface or registry
dependency graph. `docs/packages.md` describes pinned local path dependencies.
A standalone `.phoebe` file still works without a manifest,
but `import` in that mode produces a diagnostic.
