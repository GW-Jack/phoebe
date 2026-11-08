use std::process::Command;

use phoebe::{core, native, runtime::ScopeLimits, syntax};

#[test]
fn raw_program_must_pass_verification_before_it_can_be_checked() {
    let ast = syntax::parse("fn main() -> I64 = 42").unwrap();
    let mut raw = core::lower(&ast).unwrap();
    raw.functions[0].blocks[0].params.push(core::ValueId(900));
    assert!(native::compile_jit(&raw, "main").is_err());
    let unused = std::env::temp_dir().join("phoebe-invalid-checked-output");
    assert!(native::build_executable_cached(&raw, &unused, &unused).is_err());
    assert!(core::CheckedProgram::check(raw).is_err());
}

#[test]
fn nominal_sum_payload_cannot_hide_a_borrowed_reference() {
    let ast = syntax::parse("data Box { Empty, Full(value: I64) } fn main() -> I64 = 42").unwrap();
    let valid = core::lower(&ast).unwrap();
    for reference_type in [
        core::Type::RefI64,
        core::Type::RefMutI64,
        core::Type::RefBool,
        core::Type::RefMutBool,
    ] {
        let mut forged = valid.clone();
        forged.data[0].payloads[1] = Some(reference_type);
        assert!(
            core::verify(&forged).is_err(),
            "Core accepted a nominal sum containing {reference_type:?}"
        );
        assert!(core::CheckedProgram::check(forged).is_err());
    }
}

#[test]
fn checked_direct_program_runs_without_another_public_verification_boundary() {
    let ast = syntax::parse(include_str!("../examples/hello.phoebe")).unwrap();
    let checked = core::lower_checked(&ast).unwrap();
    assert_eq!(checked.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run_checked(&checked, "main", &[]).unwrap(), 42);
    assert!(native::clif_checked(&checked).unwrap().contains("return"));
    assert_eq!(
        native::compile_jit_checked(&checked, "main")
            .unwrap()
            .run(&[])
            .unwrap(),
        42
    );
}

#[test]
fn checked_retained_program_builds_and_reuses_a_cached_object_set() {
    let ast = syntax::parse(include_str!("../examples/channel_exchange.phoebe")).unwrap();
    let checked = core::lower_checked(&ast).unwrap();
    assert_eq!(checked.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run_checked(&checked, "main", &[]).unwrap(), 42);
    let limited = ScopeLimits {
        channels: 0,
        ..ScopeLimits::default()
    };
    assert_eq!(
        checked.interpret_with_limits("main", &[], limited).unwrap(),
        0
    );
    assert_eq!(
        native::run_checked_with_limits(&checked, "main", &[], limited).unwrap(),
        0
    );
    assert!(native::clif_checked(&checked).unwrap().contains("call"));

    let root = std::env::temp_dir().join(format!("phoebe-checked-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let output = root.join("app");
    let cache = root.join("cache");
    native::build_executable_checked(&checked, &output).unwrap();
    assert_eq!(Command::new(&output).output().unwrap().stdout, b"42\n");
    let cold = native::build_executable_cached_checked(&checked, &output, &cache).unwrap();
    let warm = native::build_executable_cached_checked(&checked, &output, &cache).unwrap();
    assert!(!cold.reused);
    assert!(warm.reused);
    assert_eq!(Command::new(&output).output().unwrap().stdout, b"42\n");
    std::fs::remove_dir_all(root).unwrap();
}
