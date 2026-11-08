use phoebe::runtime::ScopeLimits;
use phoebe::{core, native, syntax};
use std::process::Command;
#[test]
fn nested_product_closes_channel_and_request_rights() {
    let source = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }
data Envelope { number: I64, pair: Pair, request: Request<I64> }

fn acquire(capacity: I64) -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val (right, answer) = reply<I64>()?;
  val envelope = Envelope(7, Pair(sender, receiver), request(5, right));
  val (extra_sender, extra_receiver) = channel<I64>(capacity = capacity)?;
  extra_sender.close();
  extra_receiver.close();
  answer.close();
  envelope.close_caps();
  Result<I64, ResourceError>.Ok(42)
}

fn main() -> I64 = when (acquire(1)) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    for (capacity, expected) in [(1, 42), (0, 0)] {
        let source = source.replace("acquire(1))", &format!("acquire({capacity}))"));
        let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path = std::env::temp_dir().join(format!(
            "phoebe-nested-product-rights-{}-{capacity}",
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
fn spawn_transfers_every_product_right_to_child() {
    let source = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }
data Envelope { pair: Pair, request: Request<I64> }

fn worker(envelope: Envelope) -> I64 {
  envelope.close_caps();
  0
}

fn acquire() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val (right, answer) = reply<I64>()?;
  val envelope = Envelope(Pair(sender, receiver), request(5, right));
  spawn worker(envelope)?;
  answer.close();
  Result<I64, ResourceError>.Ok(42)
}

fn main() -> I64 = when (acquire()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    for retained in [false, true] {
        let source = if retained {
            source.replace(
                "  spawn worker(envelope)?;",
                "  yield;\n  spawn worker(envelope)?;",
            )
        } else {
            source.to_owned()
        };
        let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
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
        let path = std::env::temp_dir().join(format!(
            "phoebe-product-spawn-{}-{retained}",
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
