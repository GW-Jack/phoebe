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
fn restored_right_field_allows_whole_product_transfer() {
    let source = r#"
data Bag { label: I64, sender: Sender<I64>, receiver: Receiver<I64> }
fn work(capacity: I64) -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  var bag = Bag(41, sender, receiver);
  val removed = bag.sender;
  yield;
  val (replacement, spare) = channel<I64>(capacity = capacity)?;
  bag.sender = replacement;
  val restored = bag;
  val new_sender = restored.sender;
  val old_receiver = restored.receiver;
  removed.close();
  new_sender.close();
  old_receiver.close();
  spare.close();
  Result<I64, ResourceError>.Ok(restored.label + 1)
}
fn main() -> I64 = when (work(1)) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
fn fail() -> I64 = when (work(0)) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    core::verify(&program).unwrap();
    for (entry, expected) in [("main", 42), ("fail", 0)] {
        assert_eq!(program.interpret(entry, &[]).unwrap(), expected);
        assert_eq!(native::run(&program, entry, &[]).unwrap(), expected);
    }
    let direct = core::lower(&syntax::parse(&source.replace("  yield;\n", "")).unwrap()).unwrap();
    core::verify(&direct).unwrap();
    assert_eq!(direct.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&direct, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-restored-field-{}", std::process::id()));
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
fn live_right_field_cannot_be_overwritten() {
    let source = SOURCE.replace(
        "  val extracted = bag.sender;",
        "  val (replacement, spare) = channel<I64>(capacity = 1)?;\n  bag.sender = replacement;\n  spare.close();\n  val extracted = bag.sender;",
    );
    let error = core::lower(&syntax::parse(&source).unwrap()).unwrap_err();
    assert!(
        error.message.contains("not empty") || error.message.contains("live"),
        "{error:?}"
    );
}

#[test]
fn val_product_field_assignment_is_rejected() {
    let source = SOURCE.replace("  var bag =", "  val bag =").replace(
        "  val extracted = bag.sender;",
        "  bag.label = 42;\n  val extracted = bag.sender;",
    );
    let error = core::lower(&syntax::parse(&source).unwrap()).unwrap_err();
    assert!(
        error.message.contains("cannot assign to val bag"),
        "{error:?}"
    );
}

#[test]
fn field_assignment_respects_overlapping_loan() {
    let source = SOURCE.replace(
        "  val extracted = bag.sender;",
        "  with field = &mut bag.label { bag.label = 42; }\n  val extracted = bag.sender;",
    );
    let error = core::lower(&syntax::parse(&source).unwrap()).unwrap_err();
    assert!(error.message.contains("loan"), "{error:?}");
}

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

#[test]
fn val_product_field_moves_keep_closed_siblings_usable() {
    let source = SOURCE.replace("  var bag =", "  val bag =");
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    core::verify(&program).unwrap();
    for (entry, expected) in [("main", 42), ("fail", 0)] {
        assert_eq!(program.interpret(entry, &[]).unwrap(), expected);
        assert_eq!(native::run(&program, entry, &[]).unwrap(), expected);
    }
    let path = std::env::temp_dir().join(format!("phoebe-val-field-{}", std::process::id()));
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
fn owned_product_parameter_can_transfer_fields() {
    let source = r#"
data Bag { label: I64, sender: Sender<I64>, receiver: Receiver<I64> }
fn consume(bag: Bag) -> I64 {
  val sender = bag.sender;
  val label = bag.label;
  val receiver = bag.receiver;
  sender.close();
  receiver.close();
  label + 1
}
fn work() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  Result<I64, ResourceError>.Ok(consume(Bag(41, sender, receiver)))
}
fn main() -> I64 = when (work()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-param-field-{}", std::process::id()));
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
fn val_product_stays_immutable_after_private_storage_lowering() {
    let source = SOURCE.replace("  var bag =", "  val bag =");
    let assigned = source.replace(
        "  val extracted = bag.sender;",
        "  bag = bag;\n  val extracted = bag.sender;",
    );
    let error = core::lower(&syntax::parse(&assigned).unwrap()).unwrap_err();
    assert!(
        error.message.contains("cannot assign to val bag"),
        "{error:?}"
    );
    let borrowed = source.replace(
        "  val extracted = bag.sender;",
        "  with field = &mut bag.label { field = 42; }\n  val extracted = bag.sender;",
    );
    let error = core::lower(&syntax::parse(&borrowed).unwrap()).unwrap_err();
    assert!(
        error.message.contains("cannot mutably borrow a val"),
        "{error:?}"
    );
    let borrowed_call = source
        .replace(
            "fn work(capacity: I64)",
            "fn bump(value: &mut I64) -> I64 { value = value + 1; value }\nfn work(capacity: I64)",
        )
        .replace(
            "  val extracted = bag.sender;",
            "  bump(&mut bag.label);\n  val extracted = bag.sender;",
        );
    let error = core::lower(&syntax::parse(&borrowed_call).unwrap()).unwrap_err();
    assert!(
        error.message.contains("cannot mutably borrow a val"),
        "{error:?}"
    );
}

#[test]
fn whole_product_use_does_not_materialize_a_field_move_place() {
    let source = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }
fn consume(pair: Pair) -> I64 {
  pair.close_caps();
  42
}
fn work() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val pair = Pair(sender, receiver);
  Result<I64, ResourceError>.Ok(consume(pair))
}
fn main() -> I64 = when (work()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(!program.to_string().contains("place.init"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}
