use std::process::Command;

use phoebe::runtime::ScopeLimits;
use phoebe::{core, native, syntax};

fn assert_all_backends(source: &str, label: &str) {
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-unit-{label}-{}", std::process::id()));
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
fn unit_tasks_suspend_and_return_without_a_dummy_value() {
    let source = include_str!("../examples/unit_tasks.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let worker = program.function("worker").unwrap().1;
    let waiter = program.function("wait_for").unwrap().1;
    assert_eq!(worker.return_type, core::Type::Unit);
    assert_eq!(waiter.return_type, core::Type::Unit);
    assert!(program.to_string().contains("Unit"));
    assert_all_backends(source, "tasks");

    let limits = ScopeLimits {
        tasks: 1,
        channels: 1,
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
}

#[test]
fn unit_is_zero_payload_in_products_and_result() {
    assert_all_backends(include_str!("../examples/unit_data.phoebe"), "data");
    let option = r#"
fn main() -> I64 {
  when (Option<Unit>.Some(Unit)) {
    Option<Unit>.None -> { 0 },
    Option<Unit>.Some(value) -> { value; 42 }
  }
}
"#;
    assert_all_backends(option, "option");
}

#[test]
fn zero_width_product_field_reads_work_before_and_after_suspension() {
    let direct = r#"
data Only { blank: Unit }
fn main() -> I64 {
  var item = Only(Unit);
  item.blank;
  42
}
"#;
    assert_all_backends(direct, "direct-place");
    assert_all_backends(
        include_str!("../examples/unit_place.phoebe"),
        "retained-place",
    );
}

#[test]
fn retained_call_can_return_a_zero_width_nominal_product() {
    let source = r#"
data Only { blank: Unit }
fn close_sender(sender: Sender<I64>) -> Unit { sender.close(); }
fn wait_for(receiver: Receiver<I64>) -> Only {
  val done = receiver.receive();
  receiver.close();
  Only(Unit)
}
fn start() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  spawn close_sender(sender)?;
  val item = wait_for(receiver);
  item.blank;
  Result<I64, ResourceError>.Ok(42)
}
fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    assert_all_backends(source, "zero-product-return");
}

#[test]
fn unit_arguments_and_fields_survive_a_suspending_call() {
    let source = r#"
data Packet { first: Unit, count: I64, last: Unit }

fn close_sender(sender: Sender<I64>) -> Unit { sender.close(); }
fn pass(value: Unit) -> Unit = value

fn use_packet(packet: Packet, marker: Unit) -> I64 {
  marker;
  packet.first;
  packet.last;
  packet.count
}

fn start() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  spawn close_sender(sender)?;
  var packet = Packet(Unit, 42, Unit);
  val done = receiver.receive();
  receiver.close();
  val marker = pass(packet.first);
  Result<I64, ResourceError>.Ok(use_packet(packet, marker))
}

fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    assert_all_backends(source, "suspending-arguments");
}

#[test]
fn result_unit_keeps_resource_error_edge() {
    let source = r#"
fn close_one() -> Result<Unit, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  sender.close();
  receiver.close();
  Result<Unit, ResourceError>.Ok(Unit)
}

fn main() -> I64 {
  when (close_one()) {
    Result<Unit, ResourceError>.Ok(value) -> { 42 },
    Result<Unit, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    assert_all_backends(source, "result");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let limits = ScopeLimits {
        channels: 0,
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
}

#[test]
fn non_unit_function_still_requires_a_final_expression() {
    let error = syntax::parse("fn main() -> I64 { }").unwrap_err();
    assert!(error.message.contains("final expression"));
    assert!(error.span.end <= "fn main() -> I64 { }".len());
}
