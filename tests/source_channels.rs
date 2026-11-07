use phoebe::{core, native, syntax};
use std::process::Command;

fn source(capacity: i64) -> String {
    format!(
        r#"
fn create_and_close() -> Result<I64, ResourceError> {{
  val (sender, receiver) = channel<I64>(capacity = {capacity})?;
  sender.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(7)
}}

fn main() -> I64 {{
  when (create_and_close()) {{
    Result<I64, ResourceError>.Ok(value) -> {{ value }},
    Result<I64, ResourceError>.Err(error) -> {{ 0 }}
  }}
}}
"#
    )
}

#[test]
fn source_channel_creation_agrees_on_success_and_resource_failure() {
    for (capacity, expected) in [(1, 7), (0, 0)] {
        let ast = syntax::parse(&source(capacity)).unwrap();
        let program = core::lower(&ast).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path = std::env::temp_dir().join(format!(
            "phoebe-source-channel-{}-{capacity}",
            std::process::id()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
    }
}

#[test]
fn source_channel_rights_cannot_be_forgotten_or_closed_twice() {
    let missing_close = source(1).replace("  receiver.close();\n", "");
    let error = core::lower(&syntax::parse(&missing_close).unwrap()).unwrap_err();
    assert!(error.message.contains("normal return leaks"), "{error:?}");
    assert!(error.span.start > 0);

    let double_close = source(1).replace(
        "  receiver.close();",
        "  sender.close();\n  receiver.close();",
    );
    let error = core::lower(&syntax::parse(&double_close).unwrap()).unwrap_err();
    assert!(error.message.contains("is not live"), "{error:?}");
}

#[test]
fn source_channel_creation_requires_explicit_failure_contract() {
    let missing_question = source(1).replace("capacity = 1)?", "capacity = 1)");
    let error = syntax::parse(&missing_question).unwrap_err();
    assert!(error.message.contains("? after fallible channel creation"));

    let wrong_return = source(1).replace("-> Result<I64, ResourceError>", "-> I64");
    let error = core::lower(&syntax::parse(&wrong_return).unwrap()).unwrap_err();
    assert!(error.message.contains("requires Result<_, ResourceError>"));

    let wrong_element = source(1).replace("channel<I64>", "channel<Bool>");
    let error = syntax::parse(&wrong_element).unwrap_err();
    assert!(error.message.contains("only I64 channels"));
}
