use phoebe::{core, native, syntax};
use std::process::Command;

const PAIR: &str = "data Pair { a: I64, b: I64 }\n";

fn lower(source: &str) -> core::Program {
    core::lower(&syntax::parse(source).unwrap()).unwrap()
}

#[test]
fn two_field_destinations_agree_across_execution_paths() {
    let cases = [
        (
            "fn main() -> I64 { val pair = Pair(20, 22); pair.a + pair.b }",
            42,
        ),
        (
            "data Marker { enabled: Bool, spare: Unit }\n\
             fn main() -> I64 { val marker = Marker(true, Unit); if (marker.enabled) { 42 } else { 0 } }",
            42,
        ),
        (
            "fn second() -> Result<I64, I64> = Result<I64, I64>.Err(7)\n\
             fn build() -> Result<Pair, I64> { val pair = Pair(20, second()?); Result<Pair, I64>.Ok(pair) }\n\
             fn main() -> I64 = when (build()) {\n\
               Result<Pair, I64>.Ok(pair) -> { pair.a + pair.b },\n\
               Result<Pair, I64>.Err(error) -> { error },\n\
             }",
            7,
        ),
        (
            "fn second() -> Result<I64, I64> { yield; Result<I64, I64>.Ok(22) }\n\
             fn build() -> Result<Pair, I64> { val pair = Pair(20, second()?); Result<Pair, I64>.Ok(pair) }\n\
             fn main() -> I64 = when (build()) {\n\
               Result<Pair, I64>.Ok(pair) -> { pair.a + pair.b },\n\
               Result<Pair, I64>.Err(error) -> { error },\n\
             }",
            42,
        ),
    ];
    for (index, (body, expected)) in cases.into_iter().enumerate() {
        let program = lower(&format!("{PAIR}{body}"));
        let ir = program.to_string();
        assert!(ir.contains("dest.begin"), "{ir}");
        assert!(ir.contains("dest.publish"), "{ir}");
        if index == 0 {
            assert!(program.cost_events().is_empty());
        }
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path =
            std::env::temp_dir().join(format!("phoebe-destination-{}-{index}", std::process::id()));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
    }
}

#[test]
fn verifier_checks_destination_field_completion() {
    let program = lower(&format!(
        "{PAIR}fn main() -> I64 {{ val pair = Pair(20, 22); pair.a + pair.b }}"
    ));
    let instructions = &program.functions[0].blocks[0].instructions;
    let begin = instructions
        .iter()
        .position(|item| matches!(item.operation, core::Operation::DestBegin(_)))
        .unwrap();
    let second_field = instructions
        .iter()
        .position(|item| matches!(item.operation, core::Operation::DestField(_, 1, _)))
        .unwrap();
    let publication = instructions
        .iter()
        .position(|item| matches!(item.operation, core::Operation::DestPublish(_)))
        .unwrap();

    let mut missing = program.clone();
    missing.functions[0].blocks[0]
        .instructions
        .remove(second_field);
    assert!(
        core::verify(&missing)
            .unwrap_err()
            .contains("field was not initialized")
    );

    let mut duplicate = program.clone();
    if let core::Operation::DestField(_, index, _) =
        &mut duplicate.functions[0].blocks[0].instructions[second_field].operation
    {
        *index = 0;
    }
    assert!(
        core::verify(&duplicate)
            .unwrap_err()
            .contains("duplicate destination field")
    );

    let mut premature = program.clone();
    premature.functions[0].blocks[0]
        .instructions
        .swap(second_field, publication);
    assert!(
        core::verify(&premature)
            .unwrap_err()
            .contains("published before field initialization")
    );

    let mut missing_begin = program;
    missing_begin.functions[0].blocks[0]
        .instructions
        .remove(begin);
    assert!(core::verify(&missing_begin).is_err());
}
