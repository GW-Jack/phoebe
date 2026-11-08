use phoebe::{core, frame_layout, native, syntax};
use std::process::Command;

fn assert_standalone(program: &core::Program, expected: i64, label: &str) {
    let path =
        std::env::temp_dir().join(format!("phoebe-source-loan-{label}-{}", std::process::id()));
    native::build_executable(program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
}

#[test]
fn nested_source_reborrow_survives_yield() {
    let source = include_str!("../examples/lexical_borrow.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let ir = program.to_string();
    assert!(ir.contains("borrow.begin &mut p0"));
    assert!(ir.contains("borrow.reborrow &mut v1"));
    assert!(ir.contains("borrow.end v2"));
    assert!(ir.contains("borrow.end v1"));
    let frames = frame_layout::analyze(&program).unwrap();
    assert_eq!(frames[0].points[0].places, vec![core::PlaceId(0)]);
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "yield");
}

#[test]
fn source_loan_survives_channel_receive() {
    let source = include_str!("../examples/lexical_channel.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let exchange = program
        .functions
        .iter()
        .find(|function| function.name == "exchange")
        .unwrap();
    assert!(
        exchange
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, core::Terminator::ChannelReceive { .. }))
    );
    let frames = frame_layout::analyze(&program).unwrap();
    let exchange_frame = frames
        .iter()
        .find(|frame| program.functions[frame.function.0].name == "exchange")
        .unwrap();
    assert!(exchange_frame.place_slots.contains(&core::PlaceId(0)));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "channel");
}

#[test]
fn shared_val_borrow_can_return_a_scalar_from_with_expression() {
    let source =
        "fn main() -> I64 { val ready = true; with r = &ready { if (r) { 42 } else { 0 } } }";
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(program.to_string().contains("borrow.begin &p0"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, 42, "shared-val");
}

#[test]
fn question_failure_ends_live_loan_before_return() {
    let source = r#"
fn fail() -> Result<I64, I64> {
  var x = 41;
  with r = &mut x {
    val unused = Result<I64, I64>.Err(7)?;
    r = unused;
  };
  Result<I64, I64>.Ok(x)
}

fn main() -> I64 {
  when (fail()) {
    Result<I64, I64>.Ok(value) -> { value },
    Result<I64, I64>.Err(error) -> { error }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let ir = program.to_string();
    assert!(ir.contains("borrow.end"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 7);
    assert_standalone(&program, 7, "question");
}

#[test]
fn invalid_source_borrows_report_the_source_span() {
    let bare = "fn main() -> I64 { var x = 1; val r = &x; 0 }";
    let error = core::lower(&syntax::parse(bare).unwrap()).unwrap_err();
    assert_eq!(&bare[error.span.start..error.span.end], "&x");
    assert!(error.message.contains("requires a with binding"));

    let immutable = "fn main() -> I64 { val x = 1; with r = &mut x { r = 2; }; 0 }";
    let error = core::lower(&syntax::parse(immutable).unwrap()).unwrap_err();
    assert_eq!(&immutable[error.span.start..error.span.end], "&mut x");
    assert!(error.message.contains("cannot grant"));

    let upgrade = "fn main() -> I64 { var x = 1; with a = &x { with b = &mut a { b = 2; }; }; x }";
    let error = core::lower(&syntax::parse(upgrade).unwrap()).unwrap_err();
    assert_eq!(&upgrade[error.span.start..error.span.end], "&mut a");
    assert!(error.message.contains("exclusive reborrow"));

    let overlap = "fn main() -> I64 { var x = 1; with r = &mut x { val other = x; }; x }";
    let error = core::lower(&syntax::parse(overlap).unwrap()).unwrap_err();
    assert!(error.message.contains("read conflicts with exclusive loan"));
}
