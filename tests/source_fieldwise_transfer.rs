use phoebe::{core, native, syntax};
use std::process::Command;

const SOURCE: &str = r#"
data Bag { label: I64, sender: Sender<I64>, receiver: Receiver<I64> }

fn work(capacity: I64) -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  var bag = Bag(41, sender, receiver);
  val extracted = bag.sender;
  yield;
  val label = bag.label;
  val (extra_sender, extra_receiver) = channel<I64>(capacity = capacity)?;
  extracted.close();
  val remaining = bag.receiver;
  remaining.close();
  extra_sender.close();
  extra_receiver.close();
  Result<I64, ResourceError>.Ok(label + 1)
}

fn unwrap(result: Result<I64, ResourceError>) -> I64 = when (result) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
fn main() -> I64 = unwrap(work(1))
fn fail() -> I64 = unwrap(work(0))
"#;

#[test]
fn field_transfer_retains_closed_sibling_and_cleans_remaining_right_on_error() {
    let program = core::lower(&syntax::parse(SOURCE).unwrap()).unwrap();
    core::verify(&program).unwrap();
    let work = &program.functions[0];
    let field_takes = work
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.operation, core::Operation::TakeField(_, _)))
        .count();
    assert!(field_takes >= 3, "two source moves plus error cleanup");
    for (entry, expected) in [("main", 42), ("fail", 0)] {
        assert_eq!(program.interpret(entry, &[]).unwrap(), expected);
        assert_eq!(native::run(&program, entry, &[]).unwrap(), expected);
    }
    let path = std::env::temp_dir().join(format!("phoebe-fieldwise-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");

    let failing = SOURCE.replace(
        "main() -> I64 = unwrap(work(1))",
        "main() -> I64 = unwrap(work(0))",
    );
    let program = core::lower(&syntax::parse(&failing).unwrap()).unwrap();
    let path = std::env::temp_dir().join(format!("phoebe-fieldwise-fail-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"0\n");
}

#[test]
fn forged_second_field_move_is_rejected() {
    let mut program = core::lower(&syntax::parse(SOURCE).unwrap()).unwrap();
    let function = &mut program.functions[0];
    let duplicate = core::ValueId(function.value_types.len());
    let (block, index) = function
        .blocks
        .iter()
        .enumerate()
        .find_map(|(block, body)| {
            body.instructions
                .iter()
                .position(|instruction| {
                    matches!(instruction.operation, core::Operation::TakeField(_, _))
                })
                .map(|index| (block, index))
        })
        .unwrap();
    let mut forged = function.blocks[block].instructions[index].clone();
    function
        .value_types
        .push(function.value_types[forged.result.unwrap().0]);
    forged.result = Some(duplicate);
    function.blocks[block]
        .instructions
        .insert(index + 1, forged);
    let error = core::verify(&program).unwrap_err();
    assert!(error.contains("not live"), "{error}");
}

#[test]
fn source_cannot_move_one_right_field_twice() {
    let source = SOURCE.replace(
        "  yield;",
        "  val duplicate = bag.sender;\n  duplicate.close();\n  yield;",
    );
    let error = core::lower(&syntax::parse(&source).unwrap()).unwrap_err();
    assert!(error.message.contains("not live"), "{error:?}");
}

#[test]
fn scalar_sibling_loan_survives_partial_right_transfer() {
    let source = SOURCE.replace(
        "  val label = bag.label;",
        "  with local = &mut bag.label { local = 42; }\n  val label = bag.label;",
    );
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 43);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 43);
}
