use phoebe::{core, native, syntax};
use std::process::Command;

#[test]
fn closed_values_survive_multiple_calls_and_aot_execution() {
    let program =
        core::lower(&syntax::parse(include_str!("../examples/ownership_value.phoebe")).unwrap())
            .unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let copies = program.cost_events();
    assert!(
        copies
            .iter()
            .any(|event| event.reason == "inferred value use")
    );
    assert!(!copies.iter().any(|event| event.reason == "explicit"));

    let path = std::env::temp_dir().join(format!("phoebe-value-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"42\n");
}
