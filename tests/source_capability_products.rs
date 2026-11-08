use phoebe::{core, native, syntax};
use std::process::Command;
#[test]
fn product_owned_channels_close_across_execution_paths_and_try_cleanup() {
    let source = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }

fn acquire(capacity: I64) -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val pair = Pair(sender, receiver);
  val (extra_sender, extra_receiver) = channel<I64>(capacity = capacity)?;
  extra_sender.close();
  extra_receiver.close();
  yield;
  pair.close_caps();
  Result<I64, ResourceError>.Ok(42)
}

fn unwrap(result: Result<I64, ResourceError>) -> I64 = when (result) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}

fn main() -> I64 = unwrap(acquire(1))
fn fail() -> I64 = unwrap(acquire(0))
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let acquire = &program.functions[0];
    assert!(
        acquire
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(instruction.operation, core::Operation::CloseCaps(_)))
    );
    let failures = acquire
        .blocks
        .iter()
        .filter_map(|block| match block.terminator {
            core::Terminator::ChannelNew { failure, .. } => Some(failure),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 2);
    let cleanup = &acquire.blocks[failures[1].0].instructions;
    assert_eq!(
        cleanup
            .iter()
            .filter(|instruction| matches!(instruction.operation, core::Operation::CloseCaps(_)))
            .count(),
        1
    );
    assert!(!cleanup.iter().any(|instruction| matches!(
        instruction.operation,
        core::Operation::CloseHandle(_) | core::Operation::CloseReply(_)
    )));
    for (entry, expected) in [("main", 42), ("fail", 0)] {
        assert_eq!(program.interpret(entry, &[]).unwrap(), expected);
        assert_eq!(native::run(&program, entry, &[]).unwrap(), expected);
    }
    let path = std::env::temp_dir().join(format!(
        "phoebe-source-product-rights-{}",
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
    assert_eq!(output.stdout, b"42\n");

    let failed_source = source.replace(
        "main() -> I64 = unwrap(acquire(1))",
        "main() -> I64 = unwrap(acquire(0))",
    );
    let failed_program = core::lower(&syntax::parse(&failed_source).unwrap()).unwrap();
    let path = std::env::temp_dir().join(format!(
        "phoebe-source-product-rights-fail-{}",
        std::process::id()
    ));
    native::build_executable(&failed_program, &path).unwrap();
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
fn capability_product_rejects_copy_projection_place_and_sum_payload() {
    let base = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }
fn use_pair(pair: Pair) -> I64 {
  pair.close_caps();
  42
}

fn main() -> I64 = 0
"#;
    for (changed, message) in [
        (
            base.replace(
                "  pair.close_caps();",
                "  copy(pair);\n  pair.close_caps();",
            ),
            "copy cannot duplicate",
        ),
        (
            base.replace(
                "  pair.close_caps();",
                "  pair.sender;\n  pair.close_caps();",
            ),
            "whole-product unpack",
        ),
        (
            base.replace(
                "  pair.close_caps();",
                "  var saved = pair;\n  saved.close_caps();",
            ),
            "cannot be stored in var",
        ),
        (
            base.replace(
                "fn use_pair",
                "data MaybePair { None, Some(value: Pair) }\nfn use_pair",
            ),
            "sum variants cannot carry",
        ),
    ] {
        let error = core::lower(&syntax::parse(&changed).unwrap()).unwrap_err();
        assert!(error.message.contains(message), "{error:?}");
        assert!(error.span.start > 0, "{error:?}");
    }
}

#[test]
fn capability_product_crosses_owned_call_return_and_whole_unpack() {
    let source = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }

fn identity(pair: Pair) -> Pair = pair

fn finish(pair: Pair) -> I64 {
  val (sender, receiver) = pair.unpack();
  sender.close();
  receiver.close();
  42
}

fn acquire() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val pair = Pair(sender, receiver);
  val passed = identity(pair);
  Result<I64, ResourceError>.Ok(finish(passed))
}

fn main() -> I64 = when (acquire()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    for retained in [false, true] {
        let source = if retained {
            source.replace(
                "  val passed = identity(pair);",
                "  val passed = identity(pair);\n  yield;",
            )
        } else {
            source.to_owned()
        };
        let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
        let path = std::env::temp_dir().join(format!(
            "phoebe-source-product-call-{}-{retained}",
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
        assert_eq!(output.stdout, b"42\n");
    }
}
