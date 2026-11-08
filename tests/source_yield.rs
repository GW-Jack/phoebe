use phoebe::runtime::{NativePollTask, Scheduler};
use phoebe::{core, frame_layout, native, syntax};
use std::process::Command;

#[test]
fn source_yield_resumes_with_mutable_state_in_all_backends() {
    let source = include_str!("../examples/yield_loop.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(program.to_string().contains("yield resume"));
    let sites = program
        .cost_events()
        .into_iter()
        .filter(|event| event.kind == "task.yield")
        .collect::<Vec<_>>();
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].kind, "task.yield");
    assert_eq!(sites[0].reason, "explicit");
    assert_eq!(&source[sites[0].span.start..sites[0].span.end], "yield");
    assert_eq!(
        frame_layout::suspendable_functions(&program).unwrap(),
        [true, true]
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);

    let executable = native::compile_poll_jit(&program, "main").unwrap();
    let entry = unsafe { executable.entry() };
    let task = unsafe { NativePollTask::new(entry, executable.layout().total_words) }.unwrap();
    let mut scheduler = Scheduler::new(task, 1, 0, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.stats().yields, 42);

    let path = std::env::temp_dir().join(format!("phoebe-yield-{}", std::process::id()));
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
fn cpu_loop_checkpoints_every_1024_backedges_in_all_backends() {
    let source = r#"
fn child() -> I64 = 0

fn launch() -> Result<I64, ResourceError> {
  spawn child()?;
  Result<I64, ResourceError>.Ok(0)
}

fn main() -> I64 {
  val launched = launch();
  var count = 0;
  while (count < 2500) {
    count = count + 1;
  }
  count
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let sites = program
        .cost_events()
        .into_iter()
        .filter(|event| event.kind == "task.yield")
        .collect::<Vec<_>>();
    assert!(program.to_string().contains("yield.auto resume"));
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].kind, "task.yield");
    assert_eq!(sites[0].reason, "loop backedge");
    assert_eq!(
        &source[sites[0].span.start..sites[0].span.end],
        "count < 2500"
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 2500);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 2500);

    let executable = native::compile_poll_jit(&program, "main").unwrap();
    let entry = unsafe { executable.entry() };
    let task = unsafe { NativePollTask::new(entry, executable.layout().total_words) }.unwrap();
    let mut scheduler = Scheduler::new(task, 1, 0, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 2500);
    assert_eq!(scheduler.stats().yields, 2);

    let path = std::env::temp_dir().join(format!("phoebe-loop-budget-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"2500\n");
}

#[test]
fn single_task_loop_keeps_direct_native_path() {
    let source = include_str!("../examples/loop.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(program.cost_events().is_empty());
    assert_eq!(
        frame_layout::suspendable_functions(&program).unwrap(),
        [false, false]
    );
    assert!(!program.to_string().contains("yield.auto"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 45);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 45);

    let unrelated_spawn = format!(
        "{source}\nfn worker() -> I64 = 0\nfn unused() -> Result<I64, ResourceError> {{ spawn worker()?; Result<I64, ResourceError>.Ok(0) }}\n"
    );
    let program = core::lower(&syntax::parse(&unrelated_spawn).unwrap()).unwrap();
    assert!(
        program
            .cost_events()
            .iter()
            .all(|event| event.kind != "task.yield")
    );
    assert!(!program.runtime_context_functions()[program.function("main").unwrap().0.0]);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 45);
}

#[test]
fn automatic_checkpoint_lets_a_quick_sibling_send_first() {
    let source = r#"
fn slow(sender: Sender<I64>) -> I64 {
  var count = 0;
  while (count < 2500) {
    count = count + 1;
  }
  val outcome = sender.send(42);
  sender.close();
  when (outcome) {
    Send<I64>.Sent -> { 0 },
    Send<I64>.Closed(unsent) -> { unsent }
  }
}

fn fast(sender: Sender<I64>) -> I64 {
  val outcome = sender.send(7);
  sender.close();
  when (outcome) {
    Send<I64>.Sent -> { 0 },
    Send<I64>.Closed(unsent) -> { unsent }
  }
}

fn start() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val second = sender.fork()?;
  spawn slow(sender)?;
  spawn fast(second)?;
  val first = receiver.receive();
  val ignored = receiver.receive();
  receiver.close();
  when (first) {
    Receive<I64>.Value(value) -> { Result<I64, ResourceError>.Ok(value) },
    Receive<I64>.End -> { Result<I64, ResourceError>.Ok(0) }
  }
}

fn main() -> I64 {
  when (start()) {
    Result<I64, ResourceError>.Ok(value) -> { value },
    Result<I64, ResourceError>.Err(error) -> { 0 }
  }
}
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 7);

    let path = std::env::temp_dir().join(format!("phoebe-loop-fair-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"7\n");
}

#[test]
fn yielded_child_keeps_its_sender_until_send_and_cleanup() {
    let source = include_str!("../examples/with_wait.phoebe").replace(
        "with output = sender {",
        "with output = sender {\n    yield;",
    );
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}
