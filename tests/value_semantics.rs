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

#[test]
fn last_use_inference_respects_join_and_loop_liveness() {
    for (source, expected) in [
        ("data Pair { n: I64 } fn read(p: Pair) -> I64 = p.n
fn main() -> I64 { val p = Pair(20); val first = if (true) { read(p) } else { 0 }; first + read(p) }", 40),
        ("data Pair { n: I64 } fn read(p: Pair) -> I64 = p.n
fn main() -> I64 { val p = Pair(20); var i = 0; while (i < 2) { read(p); i = i + 1; } read(p) + 22 }", 42),
    ] {
        let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let (main_id, _) = program.function("main").unwrap();
        let operations = program.functions[main_id.0]
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter().map(|instruction| &instruction.operation))
            .collect::<Vec<_>>();
        assert_eq!(operations.iter().filter(|op| matches!(op, core::Operation::InferredCopy(_))).count(), 1);
        assert_eq!(operations.iter().filter(|op| matches!(op, core::Operation::Move(_))).count(), 1);
    }
}

#[test]
fn alternative_final_uses_transfer_without_copying() {
    let source = "data Pair { n: I64 } fn read(p: Pair) -> I64 = p.n
fn main() -> I64 { val p = Pair(42); if (true) { read(p) } else { read(p) } }";
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let (main_id, _) = program.function("main").unwrap();
    let operations = program.functions[main_id.0]
        .blocks
        .iter()
        .flat_map(|block| {
            block
                .instructions
                .iter()
                .map(|instruction| &instruction.operation)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        operations
            .iter()
            .filter(|op| matches!(op, core::Operation::Move(_)))
            .count(),
        2
    );
    assert!(
        !operations
            .iter()
            .any(|op| matches!(op, core::Operation::InferredCopy(_)))
    );
}
