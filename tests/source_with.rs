use phoebe::core::Operation;
use phoebe::runtime::ScopeLimits;
use phoebe::{core, native, syntax};
use std::process::Command;

const SOURCE: &str = include_str!("../examples/with_channel.phoebe");

#[test]
fn lexical_endpoint_cleanup_agrees_across_engines() {
    let program = core::lower(&syntax::parse(SOURCE).unwrap()).unwrap();
    let run = &program.functions[1];
    let normal_closes = run
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.operation, Operation::CloseHandle(_)))
        .count();
    // The receiver closes at the outer block exit. The transferred sender
    // has no fall-through close; failure paths still close retained rights.
    assert_eq!(normal_closes, 3);
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 7);
    let limits = ScopeLimits {
        tasks: 1,
        channels: 1,
        replies: 0,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program.interpret_with_limits("main", &[], limits).unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], limits).unwrap(),
        0
    );

    let path = std::env::temp_dir().join(format!("phoebe-with-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"7\n");
}

#[test]
fn with_rejects_non_capability_and_double_close() {
    let wrong_type = "fn main() -> I64 { with x = 1 {} 0 }";
    let error = core::lower(&syntax::parse(wrong_type).unwrap()).unwrap_err();
    assert!(
        error
            .message
            .contains("requires a communication capability"),
        "{error:?}"
    );

    let double_close = SOURCE.replace(
        "    owned.close();",
        "    owned.close();\n    owned.close();",
    );
    let error = core::lower(&syntax::parse(&double_close).unwrap()).unwrap_err();
    assert!(error.message.contains("is not live"), "{error:?}");
}

#[test]
fn nested_alias_closes_the_right_once() {
    let source = r#"
fn consume(sender: Sender<I64>) -> I64 {
  with outer = sender {
    with inner = outer {}
  }
  4
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let closes = program.functions[0]
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.operation, Operation::CloseHandle(_)))
        .count();
    assert_eq!(closes, 1);
}

#[test]
fn expression_with_returns_value_and_cleans_both_outcomes() {
    let program =
        core::lower(&syntax::parse(include_str!("../examples/with_result.phoebe")).unwrap())
            .unwrap();
    for (entry, expected) in [("main", 42), ("fail", 0)] {
        assert_eq!(program.interpret(entry, &[]).unwrap(), expected);
        assert_eq!(native::run(&program, entry, &[]).unwrap(), expected);
    }
    let path = std::env::temp_dir().join(format!("phoebe-with-result-{}", std::process::id()));
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

#[test]
fn expression_with_rejects_endpoint_escape_and_missing_tail() {
    let escape = "fn bad(sender: Sender<I64>) -> Sender<I64> = with owned = sender { owned }";
    let error = core::lower(&syntax::parse(escape).unwrap()).unwrap_err();
    assert!(
        error.message.contains("cannot return its endpoint"),
        "{error:?}"
    );

    let missing = "fn bad(sender: Sender<I64>) -> I64 { val x = with owned = sender {}; 0 }";
    let error = syntax::parse(missing).unwrap_err();
    assert!(
        error.message.contains("expected a final expression"),
        "{error:?}"
    );
}

#[test]
fn expression_with_retains_owner_across_receive_park() {
    let program =
        core::lower(&syntax::parse(include_str!("../examples/with_wait.phoebe")).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-with-wait-{}", std::process::id()));
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
