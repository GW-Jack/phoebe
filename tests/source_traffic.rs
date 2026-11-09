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
fn ten_thousand_bounded_messages_agree_across_backends() {
    for (source, name) in [
        (
            include_str!("../examples/bench_channel_throughput.phoebe"),
            "bench-channel-throughput",
        ),
        (
            include_str!("../examples/bench_typed_channel_throughput.phoebe"),
            "bench-typed-channel-throughput",
        ),
    ] {
        let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), 49_995_000);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 49_995_000);
        assert_standalone(&program, name, 49_995_000);
    }
}

#[test]
fn source_two_message_exchange_parks_and_wakes() {
    let source = include_str!("../examples/channel_exchange.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(program.data.iter().all(|data| data.name != "Reply<I64>"));
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
                    ..ScopeLimits::default()
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
                ..ScopeLimits::default()
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
                ..ScopeLimits::default()
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
    assert_eq!(exchange.place_types[1], core::Type::I64);
    assert_eq!(layout.places.len(), 1);
    assert_eq!(layout.places[0].id.0, 1);
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "channel-accumulator", 42);
}

#[test]
fn borrowed_caller_place_survives_suspended_callee_and_forwarding() {
    let source = include_str!("../examples/borrow_across_wait.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let (id, exchange) = program.function("exchange").unwrap();
    assert_eq!(
        exchange.loan_regions[0].roots,
        vec![core::LoanRoot::Place(core::PlaceId(0), true)]
    );
    let layout = native::frame_word_layouts(&program)
        .unwrap()
        .into_iter()
        .find(|layout| layout.function == id)
        .unwrap();
    assert_eq!(layout.places.len(), 1);
    assert_eq!(layout.resumes[0].places.len(), 1);
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "borrow-across-wait", 42);
    assert!(
        native::compile_poll_jit(&program, "add_after_wait")
            .err()
            .unwrap()
            .contains("external poll entry requires owned parameters")
    );
}

#[test]
fn shared_val_borrow_is_retained_through_pending_receive() {
    let source = r#"
fn close(sender: Sender<I64>) -> I64 {
  sender.close();
  0
}

fn wait(value: &I64, receiver: Receiver<I64>) -> I64 {
  val item = receiver.receive();
  receiver.close();
  value
}

fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val held = 42;
  spawn close(sender)?;
  Result<I64, ResourceError>.Ok(wait(&held, receiver))
}

fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let (id, exchange) = program.function("exchange").unwrap();
    let layout = native::frame_word_layouts(&program)
        .unwrap()
        .into_iter()
        .find(|layout| layout.function == id)
        .unwrap();
    assert_eq!(exchange.place_types.len(), 1);
    assert_eq!(layout.places.len(), 1);
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "shared-borrow-across-wait", 42);
}

#[test]
fn bool_borrow_is_retained_through_pending_receive() {
    let source = r#"
fn close(sender: Sender<I64>) -> I64 {
  sender.close();
  0
}

fn wait(flag: &mut Bool, receiver: Receiver<I64>) -> Bool {
  val item = receiver.receive();
  receiver.close();
  flag = !flag;
  flag
}

fn exchange() -> Result<Bool, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  var held = false;
  spawn close(sender)?;
  Result<Bool, ResourceError>.Ok(wait(&mut held, receiver))
}

fn main() -> I64 {
  when (exchange()) {
    Result<Bool, ResourceError>.Ok(value) -> { if (value) { 42 } else { 0 } },
    Result<Bool, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "bool-borrow-across-wait", 42);
}

#[test]
fn deadlocked_borrowed_call_releases_native_scope_resources() {
    let source = r#"
fn wait(value: &I64, receiver: Receiver<I64>) -> I64 {
  val item = receiver.receive();
  receiver.close();
  value
}

fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val held = 42;
  val answer = wait(&held, receiver);
  sender.close();
  Result<I64, ResourceError>.Ok(answer)
}

fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(
        program
            .interpret("main", &[])
            .unwrap_err()
            .contains("scope deadlocked")
    );
    let executable = native::compile_poll_jit(&program, "main").unwrap();
    let root = unsafe { NativePollTask::new(executable.entry(), executable.layout().total_words) }
        .unwrap();
    let mut scheduler = Scheduler::new(root, 1, 1, 0).unwrap();
    assert!(scheduler.run().unwrap_err().contains("scope deadlocked"));
    scheduler.verify().unwrap();

    let path = std::env::temp_dir().join(format!("phoebe-deadlock-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"phoebe runtime failed: scope deadlocked\n");
}

#[test]
fn deadlocked_nested_borrowed_calls_release_all_native_frames() {
    let source = r#"
fn inner(value: &mut I64, receiver: Receiver<I64>) -> I64 {
  val item = receiver.receive();
  receiver.close();
  value
}

fn middle(value: &mut I64, receiver: Receiver<I64>) -> I64 {
  inner(&mut value, receiver)
}

fn outer(value: &mut I64, receiver: Receiver<I64>) -> I64 {
  middle(&mut value, receiver)
}

fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  var x = 42;
  with borrowed = &mut x {
    val answer = outer(&mut borrowed, receiver);
    borrowed = answer;
  };
  sender.close();
  Result<I64, ResourceError>.Ok(x)
}

fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(
        program
            .interpret("main", &[])
            .unwrap_err()
            .contains("scope deadlocked")
    );
    let executable = native::compile_poll_jit(&program, "main").unwrap();
    let root = unsafe { NativePollTask::new(executable.entry(), executable.layout().total_words) }
        .unwrap();
    let mut scheduler = Scheduler::new(root, 1, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap_err(), "scope deadlocked");
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();

    let path = std::env::temp_dir().join(format!("phoebe-nested-deadlock-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"phoebe runtime failed: scope deadlocked\n");
}

#[test]
fn retained_parent_calls_direct_helper_with_wide_result() {
    let source = include_str!("../examples/wide_after_wait.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let (wrap, _) = program.function("wrap").unwrap();
    assert!(!phoebe::frame_layout::suspendable_functions(&program).unwrap()[wrap.0]);
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "wide-after-wait", 42);
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
fn source_nominal_message_uses_checked_typed_channel() {
    let source = r#"
data Number { value: I64, valid: Bool }

fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<Number>(capacity = 1)?;
  val sent = sender.send(Number(42, true));
  sender.close();
  when (sent) {
    Send<Number>.Sent -> {
      val received = receiver.receive();
      receiver.close();
      when (received) {
        Receive<Number>.Value(number) -> {
          if (number.valid) { Result<I64, ResourceError>.Ok(number.value) }
          else { Result<I64, ResourceError>.Ok(-1) }
        },
        Receive<Number>.End -> { Result<I64, ResourceError>.Ok(-1) }
      }
    },
    Send<Number>.Closed(number) -> {
      receiver.close();
      Result<I64, ResourceError>.Ok(number.value)
    }
  }
}

fn main() -> I64 {
  when (exchange()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { -2 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let printed = program.to_string();
    assert!(printed.contains("channel.new.typed"));
    assert!(printed.contains("channel.send.typed"));
    assert!(printed.contains("channel.receive.typed"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "typed-nominal", 42);
}

#[test]
fn source_closed_typed_send_returns_aggregate_with_owned_right() {
    let source = r#"
data Envelope { number: I64, right: ReplyTo<I64> }

fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<Envelope>(capacity = 1)?;
  val (right, reply_receiver) = reply<I64>()?;
  receiver.close();
  val sent = sender.send(Envelope(42, right));
  sender.close();
  when (sent) {
    Send<Envelope>.Sent -> {
      reply_receiver.close();
      Result<I64, ResourceError>.Ok(-1)
    },
    Send<Envelope>.Closed(envelope) -> {
      val (number, returned_right) = envelope.unpack();
      returned_right.close();
      reply_receiver.close();
      Result<I64, ResourceError>.Ok(number)
    }
  }
}

fn main() -> I64 = when (exchange()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { -2 },
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "typed-closed-right", 42);
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

#[test]
fn zero_width_unit_message_occupies_a_typed_queue_slot() {
    let source = r#"
fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<Unit>(capacity = 1)?;
  val sent = sender.send(Unit);
  sender.close();
  when (sent) {
    Send<Unit>.Sent -> {
      val received = receiver.receive();
      receiver.close();
      when (received) {
        Receive<Unit>.Value(unit) -> { Result<I64, ResourceError>.Ok(42) },
        Receive<Unit>.End -> { Result<I64, ResourceError>.Ok(-1) }
      }
    },
    Send<Unit>.Closed(unit) -> {
      receiver.close();
      Result<I64, ResourceError>.Ok(-2)
    }
  }
}
fn main() -> I64 = when (exchange()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { -3 },
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "typed-unit", 42);
}

#[test]
fn typed_producer_parks_on_full_queue_and_resumes_after_receive() {
    let source = include_str!("../examples/channel_exchange.phoebe")
        .replace("fn producer", "data Number { value: I64 }\nfn producer")
        .replace("Sender<I64>", "Sender<Number>")
        .replace("channel<I64>", "channel<Number>")
        .replace("Send<I64>", "Send<Number>")
        .replace("Receive<I64>", "Receive<Number>")
        .replace("send(20)", "send(Number(20))")
        .replace("send(22)", "send(Number(22))")
        .replace(
            "Closed(unsent) -> { unsent }",
            "Closed(unsent) -> { unsent.value }",
        )
        .replace("Ok(a + b)", "Ok(a.value + b.value)")
        .replace("Ok(a)", "Ok(a.value)");
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "typed-full-queue", 42);
}

#[test]
fn native_typed_message_moves_reply_right_inside_nominal_product() {
    let source = r#"
data Envelope { number: I64, right: ReplyTo<I64> }

fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<Envelope>(capacity = 1)?;
  val (right, answer) = reply<I64>()?;
  val sent = sender.send(Envelope(42, right));
  sender.close();
  when (sent) {
    Send<Envelope>.Sent -> {
      val received = receiver.receive();
      receiver.close();
      when (received) {
        Receive<Envelope>.Value(envelope) -> {
          val (number, moved_right) = envelope.unpack();
          moved_right.close();
          answer.close();
          Result<I64, ResourceError>.Ok(number)
        },
        Receive<Envelope>.End -> {
          answer.close();
          Result<I64, ResourceError>.Ok(-1)
        }
      }
    },
    Send<Envelope>.Closed(envelope) -> {
      val (number, returned_right) = envelope.unpack();
      returned_right.close();
      receiver.close();
      answer.close();
      Result<I64, ResourceError>.Ok(-2)
    }
  }
}

fn main() -> I64 = when (exchange()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { -3 },
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "typed-reply-right", 42);
}

#[test]
fn native_typed_message_transfers_only_the_active_tagged_right() {
    let some_source = r#"
data Envelope { maybe: Option<ReplyTo<I64>>, number: I64 }

fn exchange() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<Envelope>(capacity = 1)?;
  val (right, answer) = reply<I64>()?;
  val sent = sender.send(Envelope(Option<ReplyTo<I64>>.Some(right), 42));
  sender.close();
  when (sent) {
    Send<Envelope>.Sent -> {
      val received = receiver.receive();
      receiver.close();
      when (received) {
        Receive<Envelope>.Value(envelope) -> {
          val (maybe, number) = envelope.unpack();
          maybe.close_caps();
          answer.close();
          Result<I64, ResourceError>.Ok(number)
        },
        Receive<Envelope>.End -> {
          answer.close();
          Result<I64, ResourceError>.Ok(-1)
        }
      }
    },
    Send<Envelope>.Closed(envelope) -> {
      envelope.close_caps();
      receiver.close();
      answer.close();
      Result<I64, ResourceError>.Ok(-2)
    }
  }
}

fn main() -> I64 = when (exchange()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { -3 },
}
"#;
    let none_source = some_source.replace(
        "val sent = sender.send(Envelope(Option<ReplyTo<I64>>.Some(right), 42));",
        "right.close();\n  val sent = sender.send(Envelope(Option<ReplyTo<I64>>.None, 42));",
    );
    for (index, source) in [some_source, &none_source].into_iter().enumerate() {
        let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
        assert_standalone(&program, &format!("typed-tagged-right-{index}"), 42);
    }
}
