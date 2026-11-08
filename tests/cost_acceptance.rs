use phoebe::{core, native, syntax};
use std::process::Command;

#[test]
fn scalar_identity_has_no_phoebe_cost_or_backend_runtime_work() {
    let source = include_str!("../examples/cost/identity.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let function = &program.functions[0];
    assert!(function.blocks[0].instructions.is_empty());
    assert!(program.cost_events().is_empty());
    let clif = native::clif(&program).unwrap();
    assert!(clif.contains("return v0"), "{clif}");
    assert!(!clif.contains("call "), "{clif}");
    assert!(!clif.contains("stack_slot"), "{clif}");
    let executable = native::compile_jit(&program, "identity").unwrap();
    drop(program);
    assert_eq!(executable.run(&[73]).unwrap(), 73);
    assert_eq!(executable.run(&[42]).unwrap(), 42);
    assert!(executable.run(&[]).unwrap_err().contains("expects 1"));
}

#[test]
fn each_source_spawn_has_one_attributed_task_creation_site() {
    let source = include_str!("../examples/spawn_pair.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let events = program
        .cost_events()
        .into_iter()
        .filter(|event| event.kind == "task.create")
        .collect::<Vec<_>>();
    let sites = source
        .match_indices("spawn ")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(events.len(), sites.len());
    for (event, site) in events.iter().zip(sites) {
        assert_eq!(event.function, "start");
        assert_eq!(event.span.start, site);
        assert_eq!(event.reason, "explicit spawn");
        assert_eq!(event.type_name, "Task");
    }
}

#[test]
fn unreachable_channel_code_does_not_change_main_native_abi() {
    let source = r#"
fn unused() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  sender.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(99)
}

fn helper() -> I64 = 7
fn main() -> I64 = helper()
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(!program.runtime_context_functions()[2]);
    let path = std::env::temp_dir().join(format!("phoebe-dead-runtime-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let mut incremental = native::NativeBuildSession::new();
    assert_eq!(incremental.build_executable(&program, &path).unwrap(), 0);
    assert_eq!(incremental.build_executable(&program, &path).unwrap(), 2);
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
fn unreachable_spawn_graph_does_not_enter_standalone_binary() {
    let mut source =
        include_str!("../examples/spawn_pair.phoebe").replace("fn main()", "fn unused_main()");
    source.push_str("\nfn main() -> I64 = 7\n");
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    let path = std::env::temp_dir().join(format!("phoebe-dead-spawn-{}", std::process::id()));
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
