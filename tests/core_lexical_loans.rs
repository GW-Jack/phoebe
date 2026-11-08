use phoebe::{core, frame_layout, native, syntax};
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
    assert!(
        core::verify(&escaped)
            .unwrap_err()
            .contains("crosses return")
    );

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
fn lexical_reference_cannot_lose_identity_through_block_parameter() {
    let mut program = lexical_write();
    let function = &mut program.functions[0];
    let span = function.blocks[0].instructions[2].span;
    function
        .value_types
        .extend([core::Type::RefMutI64, core::Type::I64]);
    function.blocks[0].terminator =
        core::Terminator::Jump(core::BlockId(1), vec![core::ValueId(1)]);
    function.blocks.push(core::Block {
        params: vec![core::ValueId(4)],
        instructions: vec![core::Instruction {
            result: Some(core::ValueId(5)),
            operation: core::Operation::LoadRef(core::ValueId(4)),
            span,
        }],
        terminator: core::Terminator::Return(core::ValueId(5)),
    });
    let error = core::verify(&program).unwrap_err();
    assert!(
        error.contains("reference cannot be a block parameter"),
        "{error}"
    );
}

#[test]
fn lexical_loan_requires_matching_cfg_join_and_ends_before_return() {
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
            .contains("crosses return")
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

#[test]
fn live_reborrow_crosses_yield_with_stable_root_and_reference() {
    let mut program =
        core::lower(&syntax::parse("fn main() -> I64 { var x = 41; yield; x }").unwrap()).unwrap();
    let function = &mut program.functions[0];
    let parent = core::ValueId(function.value_types.len());
    let child = core::ValueId(parent.0 + 1);
    let value = core::ValueId(parent.0 + 2);
    let read = core::ValueId(parent.0 + 3);
    function.value_types.extend([
        core::Type::RefMutI64,
        core::Type::RefMutI64,
        core::Type::I64,
        core::Type::I64,
    ]);
    let span = function.blocks[0].instructions[0].span;
    function.blocks[0].instructions.push(core::Instruction {
        result: Some(parent),
        operation: core::Operation::BorrowBegin(core::PlaceId(0), true),
        span,
    });
    function.blocks[1].instructions[0].operation = core::Operation::Const(0);
    function.blocks[1].instructions.splice(
        0..0,
        [
            core::Instruction {
                result: Some(child),
                operation: core::Operation::BorrowReborrow(parent, true),
                span,
            },
            core::Instruction {
                result: Some(value),
                operation: core::Operation::Const(42),
                span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::StoreRef(child, value),
                span,
            },
            core::Instruction {
                result: Some(read),
                operation: core::Operation::LoadRef(child),
                span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::BorrowEnd(child),
                span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::BorrowEnd(parent),
                span,
            },
        ],
    );
    function.blocks[1].terminator = core::Terminator::Return(read);
    core::verify(&program).unwrap();
    let frames = frame_layout::analyze(&program).unwrap();
    assert!(frames[0].points[0].values.contains(&parent));
    assert!(frames[0].points[0].places.contains(&core::PlaceId(0)));
    assert!(frames[0].place_slots.contains(&core::PlaceId(0)));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-live-loan-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn live_loan_crosses_channel_receive_with_stable_root_on_both_outcomes() {
    let source = include_str!("../examples/channel_exchange.phoebe").replace(
        "val first = receiver.receive();",
        "var x = 41; val first = receiver.receive();",
    );
    for (closed, expected) in [(false, 42), (true, 0)] {
        let source = if closed {
            source.replace("spawn producer(sender)?;", "sender.close();")
        } else {
            source.clone()
        };
        let mut program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
        let exchange = program
            .functions
            .iter_mut()
            .find(|function| function.name == "exchange")
            .unwrap();
        assert_eq!(exchange.place_types[0], core::Type::I64);
        let (receive_block, value_block, end_block) = exchange
            .blocks
            .iter()
            .enumerate()
            .find_map(|(id, block)| match block.terminator {
                core::Terminator::ChannelReceive { value, end, .. } => Some((id, value, end)),
                _ => None,
            })
            .unwrap();
        let reference = core::ValueId(exchange.value_types.len());
        let value_read = core::ValueId(reference.0 + 1);
        let end_read = core::ValueId(reference.0 + 2);
        exchange
            .value_types
            .extend([core::Type::RefI64, core::Type::I64, core::Type::I64]);
        let span = exchange.blocks[receive_block].instructions[0].span;
        exchange.blocks[receive_block]
            .instructions
            .push(core::Instruction {
                result: Some(reference),
                operation: core::Operation::BorrowBegin(core::PlaceId(0), false),
                span,
            });
        for (target, read) in [(value_block, value_read), (end_block, end_read)] {
            exchange.blocks[target.0].instructions.splice(
                0..0,
                [
                    core::Instruction {
                        result: Some(read),
                        operation: core::Operation::LoadRef(reference),
                        span,
                    },
                    core::Instruction {
                        result: None,
                        operation: core::Operation::BorrowEnd(reference),
                        span,
                    },
                ],
            );
        }
        core::verify(&program).unwrap();
        let frames = frame_layout::analyze(&program).unwrap();
        let exchange_frame = frames
            .iter()
            .find(|frame| program.functions[frame.function.0].name == "exchange")
            .unwrap();
        let point = exchange_frame
            .points
            .iter()
            .find(|point| point.block.0 == receive_block)
            .unwrap();
        assert!(point.values.contains(&reference));
        assert!(point.places.contains(&core::PlaceId(0)));
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path = std::env::temp_dir().join(format!(
            "phoebe-live-receive-{}-{closed}",
            std::process::id()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
    }
}
