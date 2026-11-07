use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use phoebe::{core, native, project};

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/multi_module")
}

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

fn temp_project() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "phoebe-project-test-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    for name in ["phoebe.toml", "app.phoebe", "math.phoebe"] {
        std::fs::copy(example().join(name), path.join(name)).unwrap();
    }
    path
}

#[test]
fn project_imports_and_local_data_run_in_both_engines() {
    let loaded = project::load(&example()).unwrap();
    let program = core::lower(&loaded.ast).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn project_source_map_reports_imported_module_diagnostic() {
    let mut loaded = project::load(&example()).unwrap();
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
