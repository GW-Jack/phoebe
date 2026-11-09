use phoebe::{core, native, syntax};
use std::process::Command;

#[test]
fn partially_moved_product_survives_nested_scope_resume() {
    let source = include_str!("../examples/partial_scope_cancel.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let path = std::env::temp_dir().join(format!("phoebe-partial-scope-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
}
