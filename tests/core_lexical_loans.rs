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

fn exclusive_reborrow() -> core::Program {
    let mut program = lexical_write();
    let function = &mut program.functions[0];
    let span = function.blocks[0].instructions[2].span;
    function.value_types.push(core::Type::RefMutI64);
    function.blocks[0].instructions.insert(
        3,
        core::Instruction {
            result: Some(core::ValueId(4)),
            operation: core::Operation::BorrowReborrow(core::ValueId(1), true),
            span,
        },
    );
    for instruction in &mut function.blocks[0].instructions[5..7] {
        match &mut instruction.operation {
            core::Operation::StoreRef(reference, _) | core::Operation::LoadRef(reference) => {
                *reference = core::ValueId(4)
            }
            _ => unreachable!(),
        }
    }
    function.blocks[0].instructions.insert(
        7,
        core::Instruction {
            result: None,
            operation: core::Operation::BorrowEnd(core::ValueId(4)),
            span,
        },
    );
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

#[test]
fn exclusive_reborrow_agrees_across_engines() {
    let program = exclusive_reborrow();
    core::verify(&program).unwrap();
    assert!(program.to_string().contains("borrow.reborrow &mut v1"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-reborrow-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn reborrow_freezes_parent_and_requires_child_to_end_first() {
    let mut frozen_use = exclusive_reborrow();
    let function = &mut frozen_use.functions[0];
    let span = function.blocks[0].instructions[2].span;
    function.blocks[0].instructions.insert(
        5,
        core::Instruction {
            result: None,
            operation: core::Operation::StoreRef(core::ValueId(1), core::ValueId(2)),
            span,
        },
    );
    assert!(
        core::verify(&frozen_use)
            .unwrap_err()
            .contains("frozen parent")
    );

    let mut wrong_order = exclusive_reborrow();
    wrong_order.functions[0].blocks[0].instructions.swap(7, 8);
    assert!(
        core::verify(&wrong_order)
            .unwrap_err()
            .contains("parent loan ends before child")
    );

    let mut frozen_reborrow = exclusive_reborrow();
    let function = &mut frozen_reborrow.functions[0];
    function.value_types.push(core::Type::RefI64);
    function.blocks[0].instructions.insert(
        4,
        core::Instruction {
            result: Some(core::ValueId(5)),
            operation: core::Operation::BorrowReborrow(core::ValueId(1), false),
            span,
        },
    );
    assert!(
        core::verify(&frozen_reborrow)
            .unwrap_err()
            .contains("frozen parent")
    );
}

#[test]
fn shared_parent_only_grants_shared_children() {
    let mut program = lexical_write();
    let function = &mut program.functions[0];
    function.value_types[1] = core::Type::RefI64;
    function.blocks[0].instructions[2].operation =
        core::Operation::BorrowBegin(core::PlaceId(0), false);
    function.value_types.push(core::Type::RefI64);
    let span = function.blocks[0].instructions[2].span;
    function.blocks[0].instructions.remove(4);
    function.blocks[0].instructions.insert(
        3,
        core::Instruction {
            result: Some(core::ValueId(4)),
            operation: core::Operation::BorrowReborrow(core::ValueId(1), false),
            span,
        },
    );
    function.blocks[0].instructions[5].operation = core::Operation::LoadRef(core::ValueId(4));
    function.blocks[0].instructions.insert(
        6,
        core::Instruction {
            result: None,
            operation: core::Operation::BorrowEnd(core::ValueId(4)),
            span,
        },
    );
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 41);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 41);

    program.functions[0].value_types[4] = core::Type::RefMutI64;
    program.functions[0].blocks[0].instructions[3].operation =
        core::Operation::BorrowReborrow(core::ValueId(1), true);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("exclusive reborrow requires an exclusive parent")
    );
}

#[test]
fn exclusive_parent_can_grant_shared_child_then_regain_write_access() {
    let mut program = exclusive_reborrow();
    let function = &mut program.functions[0];
    function.value_types[4] = core::Type::RefI64;
    function.blocks[0].instructions[3].operation =
        core::Operation::BorrowReborrow(core::ValueId(1), false);
    function.blocks[0].instructions.remove(5);
    let span = function.blocks[0].instructions[2].span;
    function.blocks[0].instructions.insert(
        7,
        core::Instruction {
            result: None,
            operation: core::Operation::StoreRef(core::ValueId(1), core::ValueId(2)),
            span,
        },
    );
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 41);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 41);
}
