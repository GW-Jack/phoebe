use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use phoebe::runtime::ScopeLimits;
use phoebe::{ProjectBuildSession, ProjectCompilerSession, core, native, project, syntax};

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

fn temp_generic_project() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "phoebe-generic-project-test-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    for (name, source) in [
        (
            "phoebe.toml",
            include_str!("../examples/generic_project/phoebe.toml"),
        ),
        (
            "app.phoebe",
            include_str!("../examples/generic_project/app.phoebe"),
        ),
        (
            "math.phoebe",
            include_str!("../examples/generic_project/math.phoebe"),
        ),
        (
            "base.phoebe",
            include_str!("../examples/generic_project/base.phoebe"),
        ),
    ] {
        std::fs::write(path.join(name), source).unwrap();
    }
    path
}

#[test]
fn linked_reuse_remaps_generic_call_after_definition_insertion() {
    let path = temp_generic_project();
    let before = project::load(&path).unwrap();
    let checked = core::lower_linked_checked(&before.ast).unwrap();
    assert_eq!(checked.interpret("main", &[]).unwrap(), 42);

    let base = path.join("base.phoebe");
    let original = std::fs::read_to_string(&base).unwrap();
    std::fs::write(
        &base,
        format!("pub fn unused<T>(value: T) -> T = value\n{original}"),
    )
    .unwrap();
    let after = project::load(&path).unwrap();
    let previous = before
        .ast
        .functions
        .iter()
        .map(|function| (function.name.as_str(), function.name_span.start))
        .collect::<std::collections::HashMap<_, _>>();
    let (reuse, deltas): (Vec<_>, Vec<_>) = after
        .ast
        .functions
        .iter()
        .map(|function| match previous.get(function.name.as_str()) {
            Some(start) => (true, function.name_span.start as isize - *start as isize),
            None => (false, 0),
        })
        .unzip();
    let (updated, reused) =
        core::lower_linked_reusing(&after.ast, &before.ast, &checked, &reuse, &deltas).unwrap();
    assert!(reused > 0);
    assert_eq!(updated.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run_linked(&updated, "main", &[]).unwrap(), 42);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn linked_reuse_remaps_nominal_generic_type_arguments() {
    let body = "fn identity<T>(value: T) -> T = value\nfn main() -> I64 { val boxed = identity(Box(42)); boxed.value }";
    let before_source = format!("data Box {{ value: I64 }}\n{body}");
    let after_source = format!("data Noise {{ value: I64 }}\ndata Box {{ value: I64 }}\n{body}");
    let before = syntax::parse(&before_source).unwrap();
    let after = syntax::parse(&after_source).unwrap();
    let checked = core::lower_linked_checked(&before).unwrap();
    let delta =
        after.functions[0].name_span.start as isize - before.functions[0].name_span.start as isize;
    let (updated, reused) =
        core::lower_linked_reusing(&after, &before, &checked, &[true, true], &[delta, delta])
            .unwrap();
    assert_eq!(reused, 1);
    assert_eq!(updated.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run_linked(&updated, "main", &[]).unwrap(), 42);
}

#[test]
fn generic_project_compiler_session_reuses_checked_concrete_bodies() {
    let path = temp_generic_project();
    let mut session = ProjectCompilerSession::new();
    let (first, report) = session.compile_linked(&path).unwrap();
    assert_eq!(report.reused_core_functions, 0);
    assert_eq!(first.interpret("main", &[]).unwrap(), 42);
    let (warm, report) = session.compile_linked(&path).unwrap();
    assert!(report.load.parsed.is_empty());
    assert!(report.reused_core_functions > 0);
    assert_eq!(native::run_linked(&warm, "main", &[]).unwrap(), 42);

    let base = path.join("base.phoebe");
    let original = std::fs::read_to_string(&base).unwrap();
    std::fs::write(
        &base,
        format!("pub fn unused<T>(value: T) -> T = value\n{original}"),
    )
    .unwrap();
    let (updated, report) = session.compile_linked(&path).unwrap();
    assert_eq!(report.load.parsed, ["base"]);
    assert!(report.reused_core_functions > 0);
    assert_eq!(native::run_linked(&updated, "main", &[]).unwrap(), 42);

    std::fs::write(&base, "pub fn identity<T>(value: T) -> T = missing").unwrap();
    assert!(session.compile_linked(&path).is_err());
    std::fs::write(&base, original).unwrap();
    let (repaired, report) = session.compile_linked(&path).unwrap();
    assert_eq!(report.load.parsed, ["base"]);
    assert_eq!(repaired.interpret("main", &[]).unwrap(), 42);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn generic_project_build_session_reuses_output_and_recovers_after_edits() {
    let path = temp_generic_project();
    let executable = path.join("program");
    let mut session = ProjectBuildSession::new();
    let first = session.build(&path, &executable).unwrap();
    assert_eq!(first.compile.load.parsed.len(), 3);
    assert_eq!(first.reused_native_objects, 0);
    assert!(first.linked);
    let warm = session.build(&path, &executable).unwrap();
    assert!(warm.compile.load.parsed.is_empty());
    assert!(warm.compile.reused_core_functions > 0);
    assert_eq!(warm.reused_native_objects, 1);
    assert!(!warm.linked);

    let base = path.join("base.phoebe");
    let base_source = std::fs::read_to_string(&base).unwrap();
    std::fs::write(
        &base,
        format!("pub fn unused<T>(value: T) -> T = value\n{base_source}"),
    )
    .unwrap();
    let unused = session.build(&path, &executable).unwrap();
    assert_eq!(unused.compile.load.parsed, ["base"]);
    assert_eq!(unused.reused_native_objects, 1);
    assert!(!unused.linked);

    let app = path.join("app.phoebe");
    let original = std::fs::read_to_string(&app).unwrap();
    std::fs::write(&app, original.replace("42", "43")).unwrap();
    let edited = session.build(&path, &executable).unwrap();
    assert_eq!(edited.compile.load.parsed, ["app"]);
    assert_eq!(edited.reused_native_objects, 0);
    assert!(edited.linked);
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"43\n"
    );

    std::fs::write(&app, "fn main() -> I64 = missing").unwrap();
    assert!(session.build(&path, &executable).is_err());
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"43\n"
    );
    std::fs::write(&app, original).unwrap();
    let repaired = session.build(&path, &executable).unwrap();
    assert_eq!(repaired.compile.load.parsed, ["app"]);
    assert!(repaired.linked);
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"42\n"
    );

    std::fs::write(&executable, b"tampered").unwrap();
    let restored = session.build(&path, &executable).unwrap();
    assert_eq!(restored.reused_native_objects, 1);
    assert!(restored.linked);
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"42\n"
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn linked_disk_cache_rejects_changed_and_corrupt_inputs() {
    let path = temp_generic_project();
    let executable = path.join("program");
    let cache = path.join("cache");
    let loaded = project::load(&path).unwrap();
    let checked = core::lower_linked_checked(&loaded.ast).unwrap();
    let cold = native::build_executable_cached_linked(&checked, &executable, &cache).unwrap();
    assert!(!cold.reused);
    assert!(cold.cache_write_error.is_none());
    let warm = native::build_executable_cached_linked(&checked, &executable, &cache).unwrap();
    assert!(warm.reused);

    let base = path.join("base.phoebe");
    let base_source = std::fs::read_to_string(&base).unwrap();
    std::fs::write(
        &base,
        format!("pub fn unused<T>(value: T) -> T = value\n{base_source}"),
    )
    .unwrap();
    let with_unused = project::load(&path).unwrap();
    let with_unused = core::lower_linked_checked(&with_unused.ast).unwrap();
    assert!(
        native::build_executable_cached_linked(&with_unused, &executable, &cache)
            .unwrap()
            .reused
    );

    std::fs::write(
        &base,
        format!("pub fn unused<T>(value: T) -> T = identity(value)\n{base_source}"),
    )
    .unwrap();
    let edited_unused = project::load(&path).unwrap();
    let edited_unused = core::lower_linked_checked(&edited_unused.ast).unwrap();
    assert!(
        native::build_executable_cached_linked(&edited_unused, &executable, &cache)
            .unwrap()
            .reused
    );

    std::fs::write(&base, format!("\n{base_source}")).unwrap();
    let shifted = project::load(&path).unwrap();
    let shifted = core::lower_linked_checked(&shifted.ast).unwrap();
    assert!(
        native::build_executable_cached_linked(&shifted, &executable, &cache)
            .unwrap()
            .reused
    );

    std::fs::write(
        &base,
        format!(
            "pub fn unused<T>(value: T) -> T = value\n{}",
            base_source.replace("= value", "= unused(value)")
        ),
    )
    .unwrap();
    let reached = project::load(&path).unwrap();
    let reached = core::lower_linked_checked(&reached.ast).unwrap();
    assert!(
        !native::build_executable_cached_linked(&reached, &executable, &cache)
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

    let app = path.join("app.phoebe");
    let original = std::fs::read_to_string(&app).unwrap();
    std::fs::write(&app, original.replace("42", "43")).unwrap();
    let changed = project::load(&path).unwrap();
    let changed = core::lower_linked_checked(&changed.ast).unwrap();
    let rebuilt = native::build_executable_cached_linked(&changed, &executable, &cache).unwrap();
    assert!(!rebuilt.reused);
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"43\n"
    );

    let cache_file = cache.join("objects.v4");
    let mut bytes = std::fs::read(&cache_file).unwrap();
    let last = bytes.last_mut().unwrap();
    *last ^= 1;
    std::fs::write(&cache_file, bytes).unwrap();
    let recovered = native::build_executable_cached_linked(&changed, &executable, &cache).unwrap();
    assert!(!recovered.reused);
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"43\n"
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn retained_generic_call_runs_in_jit_and_standalone() {
    let ast = syntax::parse(
        "fn identity<T>(value: T) -> T = value\nfn main() -> I64 { yield; identity(42) }",
    )
    .unwrap();
    let linked = core::lower_linked_checked(&ast).unwrap();
    assert_eq!(linked.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run_linked(&linked, "main", &[]).unwrap(), 42);
    assert!(
        native::clif_linked(&linked)
            .unwrap()
            .contains("Phoebe generic instance")
    );
    let path = temp_generic_project();
    let executable = path.join("retained");
    native::build_executable_linked(&linked, &executable).unwrap();
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"42\n"
    );
    let mut session = native::NativeBuildSession::new();
    assert!(
        session
            .build_executable_linked_report(&linked, &executable)
            .unwrap()
            .linked
    );
    let warm = session
        .build_executable_linked_report(&linked, &executable)
        .unwrap();
    assert_eq!(warm.reused_objects, 1);
    assert!(!warm.linked);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn direct_root_spawns_a_retained_generic_child() {
    let ast = syntax::parse(
        "fn identity<T>(value: T) -> T = value\n\
         fn child() -> I64 { yield; identity(7) }\n\
         fn start() -> Result<I64, ResourceError> { spawn child()?; Result<I64, ResourceError>.Ok(42) }\n\
         fn main() -> I64 = when (start()) { Result<I64, ResourceError>.Ok(value) -> { value }, Result<I64, ResourceError>.Err(error) -> { 0 } }",
    )
    .unwrap();
    let linked = core::lower_linked_checked(&ast).unwrap();
    assert_eq!(linked.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run_linked(&linked, "main", &[]).unwrap(), 42);
    let limited = ScopeLimits {
        tasks: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        linked.interpret_with_limits("main", &[], limited).unwrap(),
        0
    );
    assert_eq!(
        native::run_linked_with_limits(&linked, "main", &[], limited).unwrap(),
        0
    );
    let path = temp_generic_project();
    let executable = path.join("spawned");
    native::build_executable_linked(&linked, &executable).unwrap();
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"42\n"
    );
    let cache = path.join("native-cache");
    assert!(
        !native::build_executable_cached_linked(&linked, &executable, &cache)
            .unwrap()
            .reused
    );
    assert!(
        native::build_executable_cached_linked(&linked, &executable, &cache)
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
    let mut session = native::NativeBuildSession::new();
    assert!(
        session
            .build_executable_linked_report(&linked, &executable)
            .unwrap()
            .linked
    );
    let warm = session
        .build_executable_linked_report(&linked, &executable)
        .unwrap();
    assert_eq!(warm.reused_objects, 2);
    assert!(!warm.linked);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn channel_exchange_calls_generic_after_park_and_wake() {
    let source = format!(
        "fn identity<T>(value: T) -> T = value\n{}",
        include_str!("../examples/channel_exchange.phoebe").replace("a + b", "identity(a + b)")
    );
    let ast = syntax::parse(&source).unwrap();
    let linked = core::lower_linked_checked(&ast).unwrap();
    assert_eq!(linked.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run_linked(&linked, "main", &[]).unwrap(), 42);
    let limited = ScopeLimits {
        tasks: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        linked.interpret_with_limits("main", &[], limited).unwrap(),
        0
    );
    assert_eq!(
        native::run_linked_with_limits(&linked, "main", &[], limited).unwrap(),
        0
    );
    let path = temp_generic_project();
    let executable = path.join("exchange");
    native::build_executable_linked(&linked, &executable).unwrap();
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"42\n"
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn generic_project_session_builds_and_reuses_retained_main() {
    let path = temp_generic_project();
    let app = path.join("app.phoebe");
    std::fs::write(
        &app,
        "import math\nfn main() -> I64 { yield; math.through(42) }",
    )
    .unwrap();
    let executable = path.join("program");
    let mut session = ProjectBuildSession::new();
    assert!(session.build(&path, &executable).unwrap().linked);
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"42\n"
    );
    let warm = session.build(&path, &executable).unwrap();
    assert_eq!(warm.reused_native_objects, 1);
    assert!(!warm.linked);
    std::fs::write(
        &app,
        "import math\nfn main() -> I64 { yield; math.through(43) }",
    )
    .unwrap();
    let edited = session.build(&path, &executable).unwrap();
    assert_eq!(edited.reused_native_objects, 0);
    assert!(edited.linked);
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"43\n"
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn retained_generic_move_preserves_channel_right_ownership() {
    let ast = syntax::parse(
        "fn identity<T>(value: T) -> T = value\n\
         fn use_right() -> Result<I64, ResourceError> {\
           val (sender, receiver) = channel<I64>(capacity = 1)?;\
           yield;\
           val moved = identity(sender);\
           moved.close(); receiver.close();\
           Result<I64, ResourceError>.Ok(42)\
         }\n\
         fn main() -> I64 = when (use_right()) {\
           Result<I64, ResourceError>.Ok(value) -> { value },\
           Result<I64, ResourceError>.Err(error) -> { 0 }\
         }",
    )
    .unwrap();
    let linked = core::lower_linked_checked(&ast).unwrap();
    assert_eq!(linked.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run_linked(&linked, "main", &[]).unwrap(), 42);
    let path = temp_generic_project();
    let executable = path.join("right");
    native::build_executable_linked(&linked, &executable).unwrap();
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"42\n"
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn retained_generic_wide_result_uses_return_buffer() {
    let ast = syntax::parse(
        "data Triple { a: I64, b: I64, c: I64 }\n\
         fn identity<T>(value: T) -> T = value\n\
         fn main() -> I64 {\n\
           yield;\n\
           val triple = identity(Triple(10, 20, 12));\n\
           triple.a + triple.b + triple.c\n\
         }",
    )
    .unwrap();
    let linked = core::lower_linked_checked(&ast).unwrap();
    assert_eq!(linked.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run_linked(&linked, "main", &[]).unwrap(), 42);
    let path = temp_generic_project();
    let executable = path.join("wide");
    native::build_executable_linked(&linked, &executable).unwrap();
    assert_eq!(
        std::process::Command::new(&executable)
            .output()
            .unwrap()
            .stdout,
        b"42\n"
    );
    std::fs::remove_dir_all(path).unwrap();
}
