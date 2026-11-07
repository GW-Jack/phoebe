use phoebe::{core, native, syntax};

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
