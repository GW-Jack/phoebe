use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use phoebe::{core, native, project};

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
    std::fs::write(&util, "fn id(value: I64) -> I64 = value\n").unwrap();
    let mut session = project::ProjectSession::new();
    session.load(&path).unwrap();
    std::fs::write(&util, "fn id(value: Bool) -> Bool = value\n").unwrap();
    let (_, report) = session.load(&path).unwrap();
    assert_eq!(report.parsed, ["util"]);
    assert_eq!(report.changed_interfaces, ["util"]);
    assert_eq!(report.affected, ["app", "math", "util"]);
    std::fs::remove_dir_all(path).unwrap();
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
