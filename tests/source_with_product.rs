use phoebe::core::Operation;
use phoebe::runtime::ScopeLimits;
use phoebe::{core, native, syntax};
use std::process::Command;

#[test]
fn with_product_closes_on_fallthrough_and_question_failure() {
    let source = include_str!("../examples/with_product.phoebe");
    for (capacity, expected) in [(1, 42), (0, 0)] {
        let source = source.replace("acquire(1))", &format!("acquire({capacity}))"));
        let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
        assert_eq!(
            program.functions[0]
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter(|instruction| matches!(instruction.operation, Operation::CloseCaps(_)))
                .count(),
            2
        );
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path = std::env::temp_dir().join(format!(
            "phoebe-with-product-{}-{capacity}",
            std::process::id()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
    }
}

#[test]
fn nested_product_alias_closes_once() {
    let source = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }
fn consume(pair: Pair) -> I64 {
  with outer = pair {
    with inner = outer {}
  }
  42
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(
        program.functions[0]
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| matches!(instruction.operation, Operation::CloseCaps(_)))
            .count(),
        1
    );
}

#[test]
fn with_product_suppresses_close_after_spawn_transfer() {
    let source = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }

fn worker(pair: Pair) -> I64 {
  pair.close_caps();
  0
}

fn acquire() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val pair = Pair(sender, receiver);
  with owned = pair {
    spawn worker(owned)?;
  }
  Result<I64, ResourceError>.Ok(42)
}

fn main() -> I64 = when (acquire()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let closes = program.functions[1]
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.operation, Operation::CloseCaps(_)))
        .count();
    assert_eq!(closes, 1); // Only the spawn rejection's `?` exit.
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let no_child = ScopeLimits {
        tasks: 1,
        channels: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program
            .interpret_with_limits("main", &[], no_child)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], no_child).unwrap(),
        0
    );
    let path =
        std::env::temp_dir().join(format!("phoebe-with-product-spawn-{}", std::process::id()));
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
fn with_product_rejects_owned_result_escape() {
    let source = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }
fn bad(pair: Pair) -> Pair = with owned = pair { owned }
"#;
    let error = core::lower(&syntax::parse(source).unwrap()).unwrap_err();
    assert!(
        error.message.contains("cannot return its endpoint"),
        "{error:?}"
    );
    assert!(error.span.start > 0);
}
