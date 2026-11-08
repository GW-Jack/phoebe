use phoebe_abi::Shapes;
use phoebe_core::{DataDef, DataId, Program, Type as ConcreteType};

use super::*;

fn source(source: &str) -> generic_core::Module {
    generic_core::lower_program(&phoebe_syntax::parse(source).unwrap()).unwrap()
}

#[test]
fn generic_identity_and_transitive_call_run_as_native_i64() {
    let generic = source("fn identity<T>(x: T) -> T = x\nfn through<U>(x: U) -> U = identity(x)");
    let concrete = Program {
        data: vec![],
        functions: vec![],
    };
    let shapes = Shapes::new(&concrete).unwrap();
    let mut planner = Planner::new(&generic, &shapes, host_target_abi().unwrap()).unwrap();
    let root = planner.plan(1, &[ConcreteType::I64]).unwrap();
    assert_eq!(planner.instances().len(), 2);
    let executable = compile_jit(&planner, root).unwrap();
    assert_eq!(executable.instance_count(), 2);
    let entry = unsafe { executable.entry() };
    let through: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(entry) };
    assert_eq!(through(41), 41);
    assert!(executable.clif().contains("call"));
}

#[test]
fn generic_branch_runs_as_native_i64() {
    let generic = source("fn choose<T>(flag: Bool, x: T) -> T = if (flag) { x } else { x }");
    let concrete = Program {
        data: vec![],
        functions: vec![],
    };
    let shapes = Shapes::new(&concrete).unwrap();
    let mut planner = Planner::new(&generic, &shapes, host_target_abi().unwrap()).unwrap();
    let root = planner.plan(0, &[ConcreteType::I64]).unwrap();
    let executable = compile_jit(&planner, root).unwrap();
    let entry = unsafe { executable.entry() };
    let choose: extern "C" fn(u8, i64) -> i64 = unsafe { std::mem::transmute(entry) };
    assert_eq!(choose(0, 9), 9);
    assert_eq!(choose(1, 13), 13);
}

#[test]
fn unit_identity_uses_no_transport_components() {
    let generic = source("fn identity<T>(x: T) -> T = x");
    let concrete = Program {
        data: vec![],
        functions: vec![],
    };
    let shapes = Shapes::new(&concrete).unwrap();
    let mut planner = Planner::new(&generic, &shapes, host_target_abi().unwrap()).unwrap();
    let root = planner.plan(0, &[ConcreteType::Unit]).unwrap();
    let executable = compile_jit(&planner, root).unwrap();
    let entry = unsafe { executable.entry() };
    let identity: extern "C" fn() = unsafe { std::mem::transmute(entry) };
    identity();
    assert!(!executable.clif().contains("stack_addr"));
}

#[test]
fn opaque_identity_emits_no_copy_or_allocation_operation() {
    let generic = source("fn identity<T>(x: T) -> T { val y = x; y }");
    let concrete = Program {
        data: vec![],
        functions: vec![],
    };
    let shapes = Shapes::new(&concrete).unwrap();
    let mut planner = Planner::new(&generic, &shapes, host_target_abi().unwrap()).unwrap();
    let root = planner.plan(0, &[ConcreteType::I64]).unwrap();
    let executable = compile_jit(&planner, root).unwrap();
    let clif = executable.clif();
    assert!(!clif.contains("stack_addr"));
    assert!(!clif.contains("load"));
    assert!(!clif.contains("store"));
    assert!(!clif.contains("call"));
    let entry = unsafe { executable.entry() };
    let identity: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(entry) };
    assert_eq!(identity(71), 71);
}

#[test]
fn wide_generic_call_uses_the_direct_return_buffer_abi() {
    let generic = source("fn identity<T>(x: T) -> T = x\nfn through<U>(x: U) -> U = identity(x)");
    let concrete = Program {
        data: vec![DataDef {
            name: "Wide".into(),
            variants: vec![],
            payloads: vec![],
            fields: vec![
                ("a".into(), ConcreteType::I64),
                ("b".into(), ConcreteType::I64),
                ("c".into(), ConcreteType::I64),
                ("d".into(), ConcreteType::I64),
                ("e".into(), ConcreteType::I64),
                ("f".into(), ConcreteType::I64),
                ("g".into(), ConcreteType::I64),
                ("h".into(), ConcreteType::I64),
                ("i".into(), ConcreteType::I64),
            ],
        }],
        functions: vec![],
    };
    let shapes = Shapes::new(&concrete).unwrap();
    let mut planner = Planner::new(&generic, &shapes, host_target_abi().unwrap()).unwrap();
    let root = planner.plan(1, &[ConcreteType::Data(DataId(0))]).unwrap();
    assert!(planner.instances()[root].key.signature.return_buffer);
    let executable = compile_jit(&planner, root).unwrap();
    assert_eq!(executable.instance_count(), 2);
    assert!(executable.clif().contains("stack_addr"));
}
