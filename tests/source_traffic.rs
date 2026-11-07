use phoebe::runtime::{NativePollTask, Scheduler, ScopeLimits};
use phoebe::{core, native, syntax};
use std::process::Command;

fn assert_standalone(program: &core::Program, name: &str, expected: i64) {
    let path = std::env::temp_dir().join(format!("phoebe-{name}-{}", std::process::id()));
    native::build_executable(program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
}

#[test]
fn source_two_message_exchange_parks_and_wakes() {
    let source = include_str!("../examples/channel_exchange.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let printed = program.to_string();
    assert!(printed.contains("channel.send.i64"));
    assert!(printed.contains("channel.receive.i64"));
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    tasks: 2,
                    channels: 1,
                    replies: 0,
                },
            )
            .unwrap(),
        42
    );
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "channel-exchange", 42);
    assert_eq!(
        native::run_with_limits(
            &program,
            "main",
            &[],
            ScopeLimits {
                tasks: 1,
                channels: 1,
                replies: 0,
            }
        )
        .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(
            &program,
            "main",
            &[],
            ScopeLimits {
                tasks: 2,
                channels: 0,
                replies: 0,
            }
        )
        .unwrap(),
        0
    );
}

#[test]
fn native_poll_exchange_agrees_on_value_and_spawn_limit() {
    let source = include_str!("../examples/channel_exchange.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let executable = native::compile_poll_jit(&program, "main").unwrap();
    for (tasks, expected) in [(2, 42), (1, 0)] {
        let root =
            unsafe { NativePollTask::new(executable.entry(), executable.layout().total_words) }
                .unwrap();
        let mut scheduler = Scheduler::new(root, tasks, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), expected);
        scheduler.verify().unwrap();
    }
}

#[test]
fn retained_mutable_place_survives_receive_without_retaining_dead_place() {
    let source = include_str!("../examples/channel_accumulator.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let (id, exchange) = program.function("exchange").unwrap();
    let layout = native::frame_word_layouts(&program)
        .unwrap()
        .into_iter()
        .find(|layout| layout.function == id)
        .unwrap();
    assert_eq!(exchange.place_types.len(), 2);
    assert_eq!(layout.places.len(), 1);
    assert_eq!(layout.places[0].id.0, 1);
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "channel-accumulator", 42);
}

#[test]
fn suspendable_functions_call_direct_helpers_without_poll_continuations() {
    let source = include_str!("../examples/channel_exchange_helper.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let plan = phoebe::frame_layout::format(&program).unwrap();
    assert!(!plan.contains("plus_one"));
    assert!(!plan.contains("combine"));
    assert!(!plan.contains("wrap"));
    assert!(!plan.contains("resource_probe"));
    let clif = native::clif(&program).unwrap();
    assert!(clif.contains("Phoebe function plus_one"));
    assert!(clif.contains("Phoebe function combine"));
    assert!(clif.contains("Phoebe function wrap"));
    assert!(clif.contains("Phoebe function resource_probe"));
    assert!(clif.contains("Phoebe poll function producer"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "channel-helper-exchange", 42);
}

#[test]
fn retained_parent_spawns_direct_child_and_resumes_on_close() {
    let source = format!(
        "{}\n{}",
        include_str!("../examples/spawn_direct_from_poll.phoebe"),
        r#"
fn unused_worker(sender: Sender<I64>) -> I64 {
  sender.close();
  0
}

fn unused_exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  spawn unused_worker(sender)?;
  val item = receiver.receive();
  receiver.close();
  Result<I64, ResourceError>.Ok(0)
}
"#
    );
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    let plan = phoebe::frame_layout::format(&program).unwrap();
    assert!(!plan.contains("worker"));
    let clif = native::clif(&program).unwrap();
    assert!(clif.contains("Phoebe spawn poll wrapper for worker"));
    assert!(!clif.contains("unused_worker"));
    assert!(!clif.contains("unused_exchange"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 7);
    let limits = ScopeLimits {
        tasks: 1,
        channels: 1,
        replies: 0,
    };
    assert_eq!(
        program.interpret_with_limits("main", &[], limits).unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], limits).unwrap(),
        0
    );
    assert_standalone(&program, "poll-spawns-direct", 7);
}

#[test]
fn source_closed_send_returns_original_value_and_receive_reports_end() {
    let source = r#"
fn inspect() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  receiver.close();
  val sent = sender.send(9);
  sender.close();
  when (sent) {
    Send<I64>.Sent -> { Result<I64, ResourceError>.Ok(0) },
    Send<I64>.Closed(unsent) -> { Result<I64, ResourceError>.Ok(unsent) }
  }
}

fn inspect_end() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  sender.close();
  val item = receiver.receive();
  receiver.close();
  when (item) {
    Receive<I64>.Value(value) -> { Result<I64, ResourceError>.Ok(value) },
    Receive<I64>.End -> { Result<I64, ResourceError>.Ok(1) }
  }
}

fn main() -> I64 {
  when (inspect()) {
    Result<I64, ResourceError>.Ok(value) -> {
      when (inspect_end()) {
        Result<I64, ResourceError>.Ok(end) -> { value + end },
        Result<I64, ResourceError>.Err(error) -> { 0 }
      }
    },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 10);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 10);
    assert_standalone(&program, "closed-send", 10);
}

#[test]
fn source_channel_methods_check_argument_shapes() {
    let source = include_str!("../examples/channel_exchange.phoebe");
    let bad_send = source.replace("sender.send(20)", "sender.send(true)");
    assert!(
        core::lower(&syntax::parse(&bad_send).unwrap())
            .unwrap_err()
            .message
            .contains("expected I64")
    );

    let bad_receive = source.replace("receiver.receive()", "receiver.receive(1)");
    assert!(
        core::lower(&syntax::parse(&bad_receive).unwrap())
            .unwrap_err()
            .message
            .contains("receive takes no arguments")
    );
}

#[test]
fn channel_outcome_types_do_not_enter_unrelated_core_programs() {
    let program = core::lower(&syntax::parse("fn main() -> I64 = 1").unwrap()).unwrap();
    assert!(program.data.is_empty());

    let source = r#"
fn main() -> I64 {
  val result = Send<I64>.Sent;
  when (result) {
    Send<I64>.Sent -> { 1 },
    Send<I64>.Closed(value) -> { value }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.data.len(), 1);
    assert_eq!(program.data[0].name, "Send<I64>");
}
