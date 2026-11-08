use phoebe::{core, native, syntax};
use std::process::Command;

fn lexical_write() -> core::Program {
    let mut program =
        core::lower(&syntax::parse("fn main() -> I64 { var x = 41; x }").unwrap()).unwrap();
    let function = &mut program.functions[0];
    let span = function.blocks[0].instructions[2].span;
    function.value_types[1] = core::Type::RefMutI64;
    function
        .value_types
        .extend([core::Type::I64, core::Type::I64]);
    function.blocks[0].instructions.splice(
        2..,
        [
            core::Instruction {
                result: Some(core::ValueId(1)),
                operation: core::Operation::BorrowBegin(core::PlaceId(0), true),
                span,
            },
            core::Instruction {
                result: Some(core::ValueId(2)),
                operation: core::Operation::Const(42),
                span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::StoreRef(core::ValueId(1), core::ValueId(2)),
                span,
            },
            core::Instruction {
                result: Some(core::ValueId(3)),
                operation: core::Operation::LoadRef(core::ValueId(1)),
                span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::BorrowEnd(core::ValueId(1)),
                span,
            },
        ],
    );
    function.blocks[0].terminator = core::Terminator::Return(core::ValueId(3));
    program
}

#[test]
fn lexical_loan_load_store_and_end_agree_across_engines() {
    let program = lexical_write();
    core::verify(&program).unwrap();
    assert!(program.to_string().contains("borrow.begin &mut p0"));
    assert!(program.to_string().contains("borrow.end v1"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-lexical-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn lexical_loan_rejects_escape_reuse_and_overlap() {
    let valid = lexical_write();
    let mut escaped = valid.clone();
    escaped.functions[0].blocks[0].instructions.pop();
    assert!(core::verify(&escaped).unwrap_err().contains("crosses exit"));

    let mut reused = valid.clone();
    let end = reused.functions[0].blocks[0].instructions.pop().unwrap();
    reused.functions[0].blocks[0].instructions.insert(3, end);
    assert!(
        core::verify(&reused)
            .unwrap_err()
            .contains("use after borrow end")
    );

    let mut overlap = valid;
    let function = &mut overlap.functions[0];
    function.value_types.push(core::Type::RefI64);
    let span = function.blocks[0].instructions[2].span;
    function.blocks[0].instructions.insert(
        3,
        core::Instruction {
            result: Some(core::ValueId(4)),
            operation: core::Operation::BorrowBegin(core::PlaceId(0), false),
            span,
        },
    );
    function.blocks[0].instructions.push(core::Instruction {
        result: None,
        operation: core::Operation::BorrowEnd(core::ValueId(4)),
        span,
    });
    assert!(
        core::verify(&overlap)
            .unwrap_err()
            .contains("overlapping lexical borrows")
    );

    let mut mutation = lexical_write();
    let function = &mut mutation.functions[0];
    let span = function.blocks[0].instructions[2].span;
    function.blocks[0].instructions.insert(
        3,
        core::Instruction {
            result: None,
            operation: core::Operation::Store(core::PlaceId(0), core::ValueId(0)),
            span,
        },
    );
    assert!(
        core::verify(&mutation)
            .unwrap_err()
            .contains("mutation conflicts with lexical loan")
    );
}

#[test]
fn lexical_loan_requires_matching_cfg_join_and_ends_before_suspension() {
    let mut branch = core::lower(
        &syntax::parse("fn main() -> I64 { var x = 41; if (true) { x } else { x } }").unwrap(),
    )
    .unwrap();
    let function = &mut branch.functions[0];
    let reference = core::ValueId(function.value_types.len());
    function.value_types.push(core::Type::RefI64);
    let span = function.blocks[1].instructions[0].span;
    function.blocks[1].instructions.push(core::Instruction {
        result: Some(reference),
        operation: core::Operation::BorrowBegin(core::PlaceId(0), false),
        span,
    });
    assert!(
        core::verify(&branch)
            .unwrap_err()
            .contains("states differ at a CFG join")
    );

    let mut suspended =
        core::lower(&syntax::parse("fn main() -> I64 { var x = 41; yield; x }").unwrap()).unwrap();
    let function = &mut suspended.functions[0];
    let reference = core::ValueId(function.value_types.len());
    function.value_types.push(core::Type::RefI64);
    let span = function.blocks[0].instructions[1].span;
    function.blocks[0].instructions.push(core::Instruction {
        result: Some(reference),
        operation: core::Operation::BorrowBegin(core::PlaceId(0), false),
        span,
    });
    assert!(
        core::verify(&suspended)
            .unwrap_err()
            .contains("crosses exit or suspension")
    );
}

#[test]
fn lexical_loan_after_yield_runs_in_retained_native_code() {
    let mut program =
        core::lower(&syntax::parse("fn main() -> I64 { var x = 41; yield; x }").unwrap()).unwrap();
    let function = &mut program.functions[0];
    function
        .value_types
        .extend([core::Type::RefMutI64, core::Type::I64, core::Type::I64]);
    let span = function.blocks[1].instructions[0].span;
    function.blocks[1].instructions.splice(
        0..0,
        [
            core::Instruction {
                result: Some(core::ValueId(2)),
                operation: core::Operation::BorrowBegin(core::PlaceId(0), true),
                span,
            },
            core::Instruction {
                result: Some(core::ValueId(3)),
                operation: core::Operation::Const(42),
                span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::StoreRef(core::ValueId(2), core::ValueId(3)),
                span,
            },
            core::Instruction {
                result: Some(core::ValueId(4)),
                operation: core::Operation::LoadRef(core::ValueId(2)),
                span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::BorrowEnd(core::ValueId(2)),
                span,
            },
        ],
    );
    function.blocks[1].terminator = core::Terminator::Return(core::ValueId(4));
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-lexical-poll-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn two_shared_lexical_loans_can_read_the_same_place() {
    let mut program =
        core::lower(&syntax::parse("fn main() -> I64 { var x = 42; x }").unwrap()).unwrap();
    let function = &mut program.functions[0];
    let span = function.blocks[0].instructions[2].span;
    function.value_types[1] = core::Type::RefI64;
    function
        .value_types
        .extend([core::Type::RefI64, core::Type::I64]);
    function.blocks[0].instructions.splice(
        2..,
        [
            core::Instruction {
                result: Some(core::ValueId(1)),
                operation: core::Operation::BorrowBegin(core::PlaceId(0), false),
                span,
            },
            core::Instruction {
                result: Some(core::ValueId(2)),
                operation: core::Operation::BorrowBegin(core::PlaceId(0), false),
                span,
            },
            core::Instruction {
                result: Some(core::ValueId(3)),
                operation: core::Operation::LoadRef(core::ValueId(1)),
                span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::BorrowEnd(core::ValueId(1)),
                span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::BorrowEnd(core::ValueId(2)),
                span,
            },
        ],
    );
    function.blocks[0].terminator = core::Terminator::Return(core::ValueId(3));
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}
