use phoebe::runtime::{NativePollTask, Scheduler};
use phoebe::{core, frame_layout, native, syntax};
use std::process::Command;

#[test]
fn source_yield_resumes_with_mutable_state_in_all_backends() {
    let source = include_str!("../examples/yield_loop.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(program.to_string().contains("yield resume"));
    let sites = program.cost_events();
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].kind, "task.yield");
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
fn yielded_child_keeps_its_sender_until_send_and_cleanup() {
    let source = include_str!("../examples/with_wait.phoebe").replace(
        "with output = sender {",
        "with output = sender {\n    yield;",
    );
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}
