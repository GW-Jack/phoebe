use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use phoebe::{ProjectCompilerSession, core, native, package, project, syntax};

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

fn temp_project() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "phoebe-project-test-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    for (name, source) in [
        (
            "phoebe.toml",
            include_str!("../examples/multi_module/phoebe.toml"),
        ),
        (
            "app.phoebe",
            include_str!("../examples/multi_module/app.phoebe"),
        ),
        (
            "math.phoebe",
            include_str!("../examples/multi_module/math.phoebe"),
        ),
    ] {
        std::fs::write(path.join(name), source).unwrap();
    }
    path
}

fn temp_package_graph() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "phoebe-package-project-test-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    for package in ["app", "math", "util"] {
        std::fs::create_dir_all(path.join(package)).unwrap();
    }
    for (name, source) in [
        (
            "app/phoebe.toml",
            include_str!("../examples/package_graph/app/phoebe.toml"),
        ),
        (
            "app/phoebe.lock",
            include_str!("../examples/package_graph/app/phoebe.lock"),
        ),
        (
            "app/app.phoebe",
            include_str!("../examples/package_graph/app/app.phoebe"),
        ),
        (
            "math/phoebe.toml",
            include_str!("../examples/package_graph/math/phoebe.toml"),
        ),
        (
            "math/api.phoebe",
            include_str!("../examples/package_graph/math/api.phoebe"),
        ),
        (
            "math/helper.phoebe",
            include_str!("../examples/package_graph/math/helper.phoebe"),
        ),
        (
            "util/phoebe.toml",
            include_str!("../examples/package_graph/util/phoebe.toml"),
        ),
        (
            "util/api.phoebe",
            include_str!("../examples/package_graph/util/api.phoebe"),
        ),
    ] {
        std::fs::write(path.join(name), source).unwrap();
    }
    path
}

#[test]
fn disk_object_cache_reuses_exact_program_and_rejects_changes_or_corruption() {
    let path = temp_project();
    let cache = path.join("native-cache");
    let executable = path.join("program");
    let mut compiler = ProjectCompilerSession::new();
    let (first, _) = compiler.compile(&path).unwrap();
    let cold = native::build_executable_cached(&first, &executable, &cache).unwrap();
    assert!(!cold.reused);
    assert!(cold.cache_write_error.is_none());
    assert!(
        native::build_executable_cached(&first, &executable, &cache)
            .unwrap()
            .reused
    );
    let math = path.join("math.phoebe");
    let source = std::fs::read_to_string(&math).unwrap();
    let changed_source = source.replace("a + b", "a + b + 1");
    std::fs::write(&math, &changed_source).unwrap();
    let (changed, _) = compiler.compile(&path).unwrap();
    let changed_build = native::build_executable_cached(&changed, &executable, &cache).unwrap();
    assert!(!changed_build.reused);
    assert!(changed_build.cache_write_error.is_none());
    let output = std::process::Command::new(&executable).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"43\n");

    std::fs::write(&math, format!("\n{changed_source}")).unwrap();
    let (shifted, _) = compiler.compile(&path).unwrap();
    assert!(
        native::build_executable_cached(&shifted, &executable, &cache)
            .unwrap()
            .reused
    );

    let cache_file = cache.join("objects.v4");
    let mut bytes = std::fs::read(&cache_file).unwrap();
    // The final eight bytes are the zero child-object count.
    let root_last = bytes.len() - 9;
    bytes[root_last] ^= 1;
    std::fs::write(&cache_file, bytes).unwrap();
    assert!(
        !native::build_executable_cached(&shifted, &executable, &cache)
            .unwrap()
            .reused
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn disk_object_cache_ignores_only_valid_unreachable_pure_leaf_bodies() {
    let path = std::env::temp_dir().join(format!(
        "phoebe-dead-cache-test-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    let cache = path.join("cache");
    let executable = path.join("program");
    let compile = |source: &str| core::lower(&syntax::parse(source).unwrap()).unwrap();
    let first = compile("fn main() -> I64 = 42\nfn dead(x: I64) -> I64 = x + 1\n");
    assert!(
        !native::build_executable_cached(&first, &executable, &cache)
            .unwrap()
            .reused
    );
    let changed = compile("fn main() -> I64 = 42\nfn dead(x: I64) -> I64 = x * 3\n");
    assert!(
        native::build_executable_cached(&changed, &executable, &cache)
            .unwrap()
            .reused
    );
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"42\n"
    );

    // Omitting the dead body from the key must never bypass Core verification.
    let mut invalid = changed.clone();
    let dead = invalid.function("dead").unwrap().0;
    invalid.functions[dead.0].blocks[0].terminator = core::Terminator::Return(core::ValueId(9999));
    assert!(native::build_executable_cached(&invalid, &executable, &cache).is_err());

    // A new call or a signature change can affect declaration and analysis
    // inputs, so both force an emitted-object rebuild.
    let with_call = compile("fn main() -> I64 = 42\nfn dead(x: I64) -> I64 = main() + x\n");
    assert!(
        !native::build_executable_cached(&with_call, &executable, &cache)
            .unwrap()
            .reused
    );
    let new_signature = compile("fn main() -> I64 = 42\nfn dead(x: Bool) -> I64 = 3\n");
    assert!(
        !native::build_executable_cached(&new_signature, &executable, &cache)
            .unwrap()
            .reused
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn locked_transitive_package_modules_compile_and_rebuild() {
    let path = temp_package_graph();
    let app = path.join("app");
    let mut session = ProjectCompilerSession::new();
    let (first, initial) = session.compile(&app).unwrap();
    assert_eq!(
        initial.load.parsed,
        ["app", "math", "math__helper", "math__util"]
    );
    assert_eq!(first.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&first, "main", &[]).unwrap(), 42);
    let executable = path.join("package-app");
    native::build_executable(&first, &executable).unwrap();
    let output = std::process::Command::new(&executable).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
    let mut native_session = native::NativeBuildSession::new();
    assert_eq!(
        native_session
            .build_executable(&first, &executable)
            .unwrap(),
        0
    );
    assert_eq!(
        native_session
            .build_executable(&first, &executable)
            .unwrap(),
        first.functions.len()
    );
    assert!(first.to_string().contains("call math__util.add"));

    let util = path.join("util/api.phoebe");
    std::fs::write(&util, "pub fn add(a: I64, b: I64) -> I64 = a + b + 1\n").unwrap();
    assert!(
        session
            .compile(&app)
            .unwrap_err()
            .contains("stale dependency pins")
    );
    package::write_lock(&app).unwrap();
    let (changed, report) = session.compile(&app).unwrap();
    assert_eq!(report.load.parsed, ["math__util"]);
    assert_eq!(report.load.affected, ["math__util"]);
    assert_eq!(report.reused_core_functions, 6);
    assert_eq!(changed.interpret("main", &[]).unwrap(), 43);
    assert_eq!(native::run(&changed, "main", &[]).unwrap(), 43);
    assert_eq!(
        native_session
            .build_executable(&changed, &executable)
            .unwrap(),
        changed.functions.len() - 1
    );
    let output = std::process::Command::new(&executable).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"43\n");

    let renamed = "pub fn add(x: I64, b: I64) -> I64 = x + b + 1\n";
    std::fs::write(&util, renamed).unwrap();
    package::write_lock(&app).unwrap();
    let (renamed_core, report) = session.compile(&app).unwrap();
    assert!(report.load.changed_interfaces.is_empty());
    assert_eq!(report.load.affected, ["math__util"]);
    assert_eq!(report.reused_core_functions, 6);
    assert_eq!(renamed_core.interpret("main", &[]).unwrap(), 43);

    std::fs::write(&util, "pub fn add(a: Bool, b: I64) -> I64 = b\n").unwrap();
    package::write_lock(&app).unwrap();
    assert!(session.compile(&app).is_err());
    std::fs::write(&util, renamed).unwrap();
    package::write_lock(&app).unwrap();
    let (restored, report) = session.compile(&app).unwrap();
    assert_eq!(report.reused_core_functions, 7);
    assert_eq!(restored.interpret("main", &[]).unwrap(), 43);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn dependency_alias_requires_import() {
    let path = temp_package_graph();
    let app = path.join("app");
    std::fs::write(
        app.join("app.phoebe"),
        "fn main() -> I64 = math.answer(20)\n",
    )
    .unwrap();
    let error = project::load(&app).err().expect("missing import must fail");
    assert!(error.contains("module `math` is not imported"), "{error}");
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn project_imports_and_local_data_run_in_both_engines() {
    let path = temp_project();
    let loaded = project::load(&path).unwrap();
    let program = core::lower(&loaded.ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn request_source_survives_project_rewriting() {
    let path = temp_project();
    std::fs::write(
        path.join("app.phoebe"),
        include_str!("../examples/request_exchange.phoebe"),
    )
    .unwrap();
    let loaded = project::load(&path).unwrap();
    let program = core::lower(&loaded.ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn private_declarations_cannot_cross_module_boundary() {
    let path = temp_project();
    let app = path.join("app.phoebe");
    let source = std::fs::read_to_string(&app).unwrap();
    std::fs::write(
        &app,
        format!("{source}\nfn reveal() -> I64 = math.value()\n"),
    )
    .unwrap();
    let error = project::load(&path).err().unwrap();
    assert!(
        error.contains("function `math.value` is not public"),
        "{error}"
    );

    let math = path.join("math.phoebe");
    let original = std::fs::read_to_string(&math).unwrap();
    std::fs::write(&math, format!("{original}\ndata Secret {{ Value }}\n")).unwrap();
    std::fs::write(
        &app,
        format!("{source}\nfn reveal(value: math.Secret) -> I64 = 0\n"),
    )
    .unwrap();
    let error = project::load(&path).err().unwrap();
    assert!(
        error.contains("type `math.Secret` is not public"),
        "{error}"
    );
    std::fs::write(
        &app,
        format!("{source}\nfn reveal() -> I64 = math.Secret.Value\n"),
    )
    .unwrap();
    let error = project::load(&path).err().unwrap();
    assert!(
        error.contains("type `math.Secret` is not public"),
        "{error}"
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn public_imported_nullary_variant_can_be_constructed() {
    let path = temp_project();
    let app = path.join("app.phoebe");
    let source = std::fs::read_to_string(&app).unwrap();
    std::fs::write(
        &app,
        format!("{source}\nfn missing() -> I64 = math.unwrap(math.Maybe.None)\n"),
    )
    .unwrap();
    let loaded = project::load(&path).unwrap();
    let program = core::lower(&loaded.ast).unwrap();
    assert_eq!(program.interpret("app.missing", &[]).unwrap(), 0);
    assert_eq!(native::run(&program, "app.missing", &[]).unwrap(), 0);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn private_signature_edit_does_not_change_public_interface() {
    let path = temp_project();
    let mut session = project::ProjectSession::new();
    let (loaded, _) = session.load(&path).unwrap();
    let math = loaded
        .interfaces
        .iter()
        .find(|interface| interface.name == "math")
        .unwrap();
    assert!(
        !math
            .functions
            .iter()
            .any(|function| function.name == "math.value")
    );
    let source_path = path.join("math.phoebe");
    let source = std::fs::read_to_string(&source_path).unwrap();
    std::fs::write(
        &source_path,
        source.replace("fn value() -> I64", "fn value(unused: I64) -> I64"),
    )
    .unwrap();
    let (_, report) = session.load(&path).unwrap();
    assert_eq!(report.parsed, ["math"]);
    assert!(report.changed_interfaces.is_empty());
    assert_eq!(report.affected, ["math"]);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn exported_signature_tracks_private_nominal_layout() {
    let path = temp_project();
    let math_path = path.join("math.phoebe");
    let original = std::fs::read_to_string(&math_path).unwrap();
    let exposed = format!(
        "{original}\ndata Hidden {{ Value(value: I64) }}\npub fn make_hidden() -> Hidden = Hidden.Value(1)\n"
    );
    std::fs::write(&math_path, &exposed).unwrap();
    let mut session = project::ProjectSession::new();
    let (loaded, _) = session.load(&path).unwrap();
    let interface = loaded
        .interfaces
        .iter()
        .find(|interface| interface.name == "math")
        .unwrap();
    assert!(interface.data.iter().any(|data| data.name == "math.Hidden"));

    std::fs::write(
        &math_path,
        exposed.replace("Value(value: I64)", "Value(value: Bool)"),
    )
    .unwrap();
    let (_, report) = session.load(&path).unwrap();
    assert_eq!(report.changed_interfaces, ["math"]);
    assert_eq!(report.affected, ["app", "math"]);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn project_session_reuses_parses_and_invalidates_by_exact_interface() {
    let path = temp_project();
    let mut session = project::ProjectSession::new();
    let (loaded, initial) = session.load(&path).unwrap();
    assert_eq!(initial.parsed, ["app", "math"]);
    assert!(initial.reused.is_empty());
    assert_eq!(initial.changed_interfaces, ["app", "math"]);
    assert_eq!(initial.affected, ["app", "math"]);
    assert_eq!(
        core::lower(&loaded.ast)
            .unwrap()
            .interpret("main", &[])
            .unwrap(),
        42
    );

    let (_, unchanged) = session.load(&path).unwrap();
    assert!(unchanged.parsed.is_empty());
    assert_eq!(unchanged.reused, ["app", "math"]);
    assert!(unchanged.changed_interfaces.is_empty());
    assert!(unchanged.affected.is_empty());

    let math = path.join("math.phoebe");
    let original = std::fs::read_to_string(&math).unwrap();
    let body_edit = original.replace("a + b", "a + b + 0");
    std::fs::write(&math, &body_edit).unwrap();
    let (loaded, body) = session.load(&path).unwrap();
    assert_eq!(body.parsed, ["math"]);
    assert_eq!(body.reused, ["app"]);
    assert!(body.changed_interfaces.is_empty());
    assert_eq!(body.affected, ["math"]);
    assert_eq!(
        core::lower(&loaded.ast)
            .unwrap()
            .interpret("main", &[])
            .unwrap(),
        42
    );

    std::fs::write(&math, "@").unwrap();
    assert!(session.load(&path).is_err());
    std::fs::write(&math, &body_edit).unwrap();
    let (_, restored) = session.load(&path).unwrap();
    assert!(restored.parsed.is_empty());
    assert_eq!(restored.reused, ["app", "math"]);

    let signature_edit = body_edit.replace("fn add(a: I64, b: I64)", "fn add(a: I64, b: Bool)");
    std::fs::write(&math, signature_edit).unwrap();
    let (_, signature) = session.load(&path).unwrap();
    assert_eq!(signature.parsed, ["math"]);
    assert_eq!(signature.changed_interfaces, ["math"]);
    assert_eq!(signature.affected, ["app", "math"]);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn project_interface_tracks_nominal_layout_changes() {
    let path = temp_project();
    let mut session = project::ProjectSession::new();
    session.load(&path).unwrap();
    let math = path.join("math.phoebe");
    let original = std::fs::read_to_string(&math).unwrap();
    std::fs::write(
        &math,
        original.replace("Some(value: I64)", "Some(value: Bool)"),
    )
    .unwrap();
    let (_, report) = session.load(&path).unwrap();
    assert_eq!(report.parsed, ["math"]);
    assert_eq!(report.changed_interfaces, ["math"]);
    assert_eq!(report.affected, ["app", "math"]);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn project_interface_invalidation_reaches_transitive_importers() {
    let path = temp_project();
    let manifest = path.join("phoebe.toml");
    let source = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        source.replace(
            "math = \"math.phoebe\"",
            "math = \"math.phoebe\"\nutil = \"util.phoebe\"",
        ),
    )
    .unwrap();
    let math = path.join("math.phoebe");
    let source = std::fs::read_to_string(&math).unwrap();
    std::fs::write(&math, format!("import util\n{source}")).unwrap();
    let util = path.join("util.phoebe");
    std::fs::write(&util, "pub fn id(value: I64) -> I64 = value\n").unwrap();
    let mut session = project::ProjectSession::new();
    session.load(&path).unwrap();
    std::fs::write(&util, "pub fn id(value: Bool) -> Bool = value\n").unwrap();
    let (_, report) = session.load(&path).unwrap();
    assert_eq!(report.parsed, ["util"]);
    assert_eq!(report.changed_interfaces, ["util"]);
    assert_eq!(report.affected, ["app", "math", "util"]);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn compiler_session_reuses_checked_core_bodies_and_rebases_spans() {
    let path = temp_project();
    let mut session = ProjectCompilerSession::new();
    let (first, initial) = session.compile(&path).unwrap();
    assert_eq!(initial.reused_core_functions, 0);
    let count = first.functions.len();
    let (unchanged, report) = session.compile(&path).unwrap();
    assert_eq!(report.reused_core_functions, count);
    assert_eq!(unchanged.interpret("main", &[]).unwrap(), 42);

    let math = path.join("math.phoebe");
    let source = std::fs::read_to_string(&math).unwrap();
    std::fs::write(&math, source.replace("a + b", "a + b + 0")).unwrap();
    let (edited, report) = session.compile(&path).unwrap();
    assert_eq!(report.load.affected, ["math"]);
    assert_eq!(report.reused_core_functions, count - 1);
    assert_eq!(edited.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&edited, "main", &[]).unwrap(), 42);
    let fresh = project::load(&path).unwrap();
    let fresh_core = core::lower(&fresh.ast).unwrap();
    assert_eq!(edited.to_string(), fresh_core.to_string());
    let old_span = first
        .functions
        .iter()
        .find(|function| function.name == "main")
        .unwrap()
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .next()
        .unwrap()
        .span;
    let new_span = edited
        .functions
        .iter()
        .find(|function| function.name == "main")
        .unwrap()
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .next()
        .unwrap()
        .span;
    assert_eq!(new_span.start - old_span.start, 4);
    assert_eq!(new_span.end - old_span.end, 4);

    let invalid = std::fs::read_to_string(&math)
        .unwrap()
        .replace("fn add(a: I64, b: I64)", "fn add(a: I64, b: Bool)");
    std::fs::write(&math, invalid).unwrap();
    assert!(session.compile(&path).is_err());
    std::fs::write(&math, source.replace("a + b", "a + b + 0")).unwrap();
    let (restored, report) = session.compile(&path).unwrap();
    assert!(report.load.parsed.is_empty());
    assert_eq!(report.reused_core_functions, count);
    assert_eq!(restored.interpret("main", &[]).unwrap(), 42);

    let source = std::fs::read_to_string(&math).unwrap();
    let earlier_edit = source.replace("Maybe.None -> { 0 }", "Maybe.None -> { 0 + 0 }");
    std::fs::write(&math, &earlier_edit).unwrap();
    let (shifted, report) = session.compile(&path).unwrap();
    assert_eq!(report.reused_core_functions, count - 1);
    let fresh = project::load(&path).unwrap();
    assert_eq!(
        shifted.to_string(),
        core::lower(&fresh.ast).unwrap().to_string()
    );

    std::fs::write(&math, format!("{source}\npub data Extra {{ A }}\n")).unwrap();
    let (layout, report) = session.compile(&path).unwrap();
    assert_eq!(report.load.affected, ["app", "math"]);
    assert_eq!(report.reused_core_functions, 0);
    assert_eq!(layout.interpret("main", &[]).unwrap(), 42);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn core_rejects_stale_reuse_hints() {
    let old_ast = syntax::parse("fn main() -> I64 = 1\n").unwrap();
    let old_core = core::lower(&old_ast).unwrap();
    let changed_ast = syntax::parse("fn main() -> I64 = 2\n").unwrap();
    let (changed, reused) =
        core::lower_reusing(&changed_ast, &old_ast, &old_core, &[true], &[0]).unwrap();
    assert_eq!(reused, 0);
    assert_eq!(changed.interpret("main", &[]).unwrap(), 2);

    let shifted_ast = syntax::parse("  fn main() -> I64 = 1\n").unwrap();
    let (shifted, reused) =
        core::lower_reusing(&shifted_ast, &old_ast, &old_core, &[true], &[2]).unwrap();
    assert_eq!(reused, 1);
    assert_eq!(shifted.interpret("main", &[]).unwrap(), 1);
}

#[test]
fn adding_or_removing_spawn_invalidates_loop_lowering_reuse() {
    let before = r#"
fn worker() -> I64 = 0
fn launch() -> Result<I64, ResourceError> = Result<I64, ResourceError>.Ok(0)
fn count() -> I64 {
  var n = 0;
  while (n < 3) { n = n + 1; }
  n
}
fn main() -> I64 { val launched = launch(); count() }
fn isolated() -> I64 {
  var n = 0;
  while (n < 3) { n = n + 1; }
  n
}
"#;
    let after = before.replace(
        "fn launch() -> Result<I64, ResourceError> = Result<I64, ResourceError>.Ok(0)",
        "fn launch() -> Result<I64, ResourceError> { spawn worker()?; Result<I64, ResourceError>.Ok(0) }",
    );
    let before_ast = syntax::parse(before).unwrap();
    let before_core = core::lower(&before_ast).unwrap();
    assert!(before_core.cost_events().is_empty());
    let after_ast = syntax::parse(&after).unwrap();
    let hints = vec![true; before_ast.functions.len()];
    let offset =
        after.find("fn count()").unwrap() as isize - before.find("fn count()").unwrap() as isize;
    let deltas = vec![0, 0, offset, offset, offset];
    let (after_core, reused) =
        core::lower_reusing(&after_ast, &before_ast, &before_core, &hints, &deltas).unwrap();
    assert_eq!(reused, 1);
    assert!(
        after_core
            .cost_events()
            .iter()
            .any(|site| site.reason == "loop backedge")
    );
    let reverse_deltas = deltas.iter().map(|delta| -*delta).collect::<Vec<_>>();
    let (restored, reused) = core::lower_reusing(
        &before_ast,
        &after_ast,
        &after_core,
        &hints,
        &reverse_deltas,
    )
    .unwrap();
    assert_eq!(reused, 1);
    assert!(restored.cost_events().is_empty());
}

#[test]
fn project_source_map_reports_imported_module_diagnostic() {
    let path = temp_project();
    let mut loaded = project::load(&path).unwrap();
    let math = loaded
        .ast
        .functions
        .iter_mut()
        .find(|function| function.name == "math.add")
        .unwrap();
    math.body.tail.as_mut().unwrap().kind = phoebe::syntax::ExprKind::Name("missing".into());
    let error = core::lower(&loaded.ast).unwrap_err();
    let rendered = loaded.render(&error);
    assert!(rendered.contains("math.phoebe:"), "{rendered}");
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn project_rejects_unimported_module_call_at_source_location() {
    let path = temp_project();
    let app = path.join("app.phoebe");
    let source = std::fs::read_to_string(&app).unwrap();
    std::fs::write(&app, source.replace("import math\n", "")).unwrap();
    let error = project::load(&path).err().unwrap();
    assert!(error.contains("app.phoebe:2:"), "{error}");
    assert!(error.contains("module `math` is not imported"), "{error}");
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn project_rejects_import_cycle() {
    let path = temp_project();
    let manifest = path.join("phoebe.toml");
    let source = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        source.replace(
            "math = \"math.phoebe\"",
            "math = \"math.phoebe\"\nutil = \"util.phoebe\"",
        ),
    )
    .unwrap();
    std::fs::write(
        path.join("util.phoebe"),
        "import math\nfn value() -> I64 = math.value()",
    )
    .unwrap();
    let math = path.join("math.phoebe");
    let source = std::fs::read_to_string(&math).unwrap();
    std::fs::write(&math, format!("import util\n{source}")).unwrap();
    let error = project::load(&path).err().unwrap();
    assert!(error.contains("module import cycle"), "{error}");
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn project_qualifies_local_spawn_target_and_endpoint_type() {
    let path = temp_project();
    std::fs::write(
        path.join("app.phoebe"),
        r#"
fn worker(sender: Sender<I64>) -> I64 {
  sender.close();
  0
}

fn start() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  spawn worker(sender)?;
  receiver.close();
  Result<I64, ResourceError>.Ok(7)
}

fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
    )
    .unwrap();
    let loaded = project::load(&path).unwrap();
    let program = core::lower(&loaded.ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn project_qualifies_result_fields_with_imported_nominal_arguments() {
    let path = temp_project();
    std::fs::write(
        path.join("app.phoebe"),
        r#"
import math
data Boxed { result: Result<math.Maybe, Bool> }

fn main() -> I64 {
  val boxed = Boxed(Result<math.Maybe, Bool>.Ok(math.wrap()));
  when (boxed.result) {
    Result<math.Maybe, Bool>.Ok(value) -> { math.unwrap(value) },
    Result<math.Maybe, Bool>.Err(error) -> { 0 },
  }
}
"#,
    )
    .unwrap();
    let loaded = project::load(&path).unwrap();
    let program = core::lower(&loaded.ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 22);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 22);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn imported_spawn_call_keeps_lexical_scope() {
    let path = temp_project();
    std::fs::write(
        path.join("math.phoebe"),
        r#"
fn child() -> I64 = 7
pub fn launch() -> Result<I64, ResourceError> {
  spawn child()?;
  Result<I64, ResourceError>.Ok(1)
}
"#,
    )
    .unwrap();
    std::fs::write(
        path.join("app.phoebe"),
        r#"
import math
fn start() -> Result<I64, ResourceError> {
  scope { val unused = math.launch()?; }
  Result<I64, ResourceError>.Ok(42)
}
fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#,
    )
    .unwrap();
    let loaded = project::load(&path).unwrap();
    let program = core::lower(&loaded.ast).unwrap();
    core::verify(&program).unwrap();
    assert!(program.to_string().contains("scope.enter"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    std::fs::remove_dir_all(path).unwrap();
}
