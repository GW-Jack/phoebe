use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, Function, Instruction, Operation, Program, Terminator, Type, ValueId,
};
use phoebe::native;
use phoebe::runtime::ScopeLimits;
use std::process::Command;

fn span() -> Span {
    Span { start: 0, end: 0 }
}

fn instruction(result: Option<usize>, operation: Operation) -> Instruction {
    Instruction {
        result: result.map(ValueId),
        operation,
        span: span(),
    }
}

fn limits(replies: usize) -> ScopeLimits {
    ScopeLimits {
        replies,
        ..ScopeLimits::default()
    }
}

fn assert_standalone(program: &Program, label: &str, expected: i64) {
    let path = std::env::temp_dir().join(format!("phoebe-reply-{label}-{}", std::process::id()));
    native::build_executable(program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{expected}\n")
    );
}

fn completed_reply() -> Program {
    Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![
                Type::ReplyToI64,
                Type::ReplyReceiverI64,
                Type::ResourceError,
                Type::I64,
                Type::I64,
                Type::I64,
                Type::I64,
                Type::I64,
            ],
            place_types: vec![],
            loan_regions: vec![],
            blocks: vec![
                Block {
                    params: vec![],
                    instructions: vec![],
                    terminator: Terminator::ReplyNew {
                        success: BlockId(1),
                        failure: BlockId(2),
                    },
                },
                Block {
                    params: vec![ValueId(0), ValueId(1)],
                    instructions: vec![instruction(Some(3), Operation::Const(42))],
                    terminator: Terminator::ReplyComplete {
                        right: ValueId(0),
                        value: ValueId(3),
                        delivered: BlockId(3),
                        closed: BlockId(4),
                    },
                },
                Block {
                    params: vec![ValueId(2)],
                    instructions: vec![instruction(Some(4), Operation::Const(0))],
                    terminator: Terminator::Return(ValueId(4)),
                },
                Block {
                    params: vec![],
                    instructions: vec![],
                    terminator: Terminator::ReplyReceive {
                        receiver: ValueId(1),
                        value: BlockId(5),
                        end: BlockId(6),
                    },
                },
                Block {
                    params: vec![],
                    instructions: vec![
                        instruction(None, Operation::CloseReply(ValueId(1))),
                        instruction(Some(6), Operation::Const(0)),
                    ],
                    terminator: Terminator::Return(ValueId(6)),
                },
                Block {
                    params: vec![ValueId(5)],
                    instructions: vec![],
                    terminator: Terminator::Return(ValueId(5)),
                },
                Block {
                    params: vec![],
                    instructions: vec![instruction(Some(7), Operation::Const(0))],
                    terminator: Terminator::Return(ValueId(7)),
                },
            ],
        }],
    }
}

#[test]
fn checked_core_reply_completion_consumes_both_rights() {
    let program = completed_reply();
    core::verify(&program).unwrap();
    assert!(program.to_string().contains("reply.complete.i64"));
    assert_eq!(
        program
            .interpret_with_limits("main", &[], limits(1))
            .unwrap(),
        42
    );
    assert_eq!(
        program
            .interpret_with_limits("main", &[], limits(0))
            .unwrap(),
        0
    );
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_eq!(
        native::run_with_limits(&program, "main", &[], limits(0)).unwrap(),
        0
    );
    assert_standalone(&program, "complete", 42);
}

#[test]
fn verifier_rejects_leaked_and_twice_closed_reply_capabilities() {
    let mut program = completed_reply();
    program.functions[0].blocks[4].instructions.remove(0);
    let error = core::verify(&program).unwrap_err();
    assert!(error.contains("leaks capabilities"), "{error}");

    let mut program = completed_reply();
    program.functions[0].blocks[4]
        .instructions
        .insert(1, instruction(None, Operation::CloseReply(ValueId(1))));
    let error = core::verify(&program).unwrap_err();
    assert!(error.contains("not live"), "{error}");
}

#[test]
fn canceled_reply_takes_closed_edge_in_direct_native_code() {
    let program = Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![
                Type::ReplyToI64,
                Type::ReplyReceiverI64,
                Type::ResourceError,
                Type::I64,
                Type::I64,
                Type::I64,
                Type::I64,
            ],
            place_types: vec![],
            loan_regions: vec![],
            blocks: vec![
                Block {
                    params: vec![],
                    instructions: vec![],
                    terminator: Terminator::ReplyNew {
                        success: BlockId(1),
                        failure: BlockId(2),
                    },
                },
                Block {
                    params: vec![ValueId(0), ValueId(1)],
                    instructions: vec![
                        instruction(None, Operation::CloseReply(ValueId(1))),
                        instruction(Some(3), Operation::Const(42)),
                    ],
                    terminator: Terminator::ReplyComplete {
                        right: ValueId(0),
                        value: ValueId(3),
                        delivered: BlockId(3),
                        closed: BlockId(4),
                    },
                },
                Block {
                    params: vec![ValueId(2)],
                    instructions: vec![instruction(Some(6), Operation::Const(0))],
                    terminator: Terminator::Return(ValueId(6)),
                },
                Block {
                    params: vec![],
                    instructions: vec![instruction(Some(4), Operation::Const(0))],
                    terminator: Terminator::Return(ValueId(4)),
                },
                Block {
                    params: vec![],
                    instructions: vec![instruction(Some(5), Operation::Const(42))],
                    terminator: Terminator::Return(ValueId(5)),
                },
            ],
        }],
    };
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "canceled", 42);
}

#[test]
fn closed_right_makes_reply_receiver_observe_end() {
    let program = Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![
                Type::ReplyToI64,
                Type::ReplyReceiverI64,
                Type::ResourceError,
                Type::I64,
                Type::I64,
                Type::I64,
            ],
            place_types: vec![],
            loan_regions: vec![],
            blocks: vec![
                Block {
                    params: vec![],
                    instructions: vec![],
                    terminator: Terminator::ReplyNew {
                        success: BlockId(1),
                        failure: BlockId(2),
                    },
                },
                Block {
                    params: vec![ValueId(0), ValueId(1)],
                    instructions: vec![instruction(None, Operation::CloseReply(ValueId(0)))],
                    terminator: Terminator::ReplyReceive {
                        receiver: ValueId(1),
                        value: BlockId(3),
                        end: BlockId(4),
                    },
                },
                Block {
                    params: vec![ValueId(2)],
                    instructions: vec![instruction(Some(3), Operation::Const(0))],
                    terminator: Terminator::Return(ValueId(3)),
                },
                Block {
                    params: vec![ValueId(4)],
                    instructions: vec![],
                    terminator: Terminator::Return(ValueId(4)),
                },
                Block {
                    params: vec![],
                    instructions: vec![instruction(Some(5), Operation::Const(42))],
                    terminator: Terminator::Return(ValueId(5)),
                },
            ],
        }],
    };
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "ended", 42);
}

fn spawned_reply() -> Program {
    Program {
        data: vec![],
        functions: vec![
            Function {
                name: "main".into(),
                param_modes: vec![],
                return_type: Type::I64,
                value_types: vec![
                    Type::ReplyToI64,
                    Type::ReplyReceiverI64,
                    Type::ResourceError,
                    Type::ResourceError,
                    Type::I64,
                    Type::I64,
                    Type::I64,
                    Type::I64,
                ],
                place_types: vec![],
                loan_regions: vec![],
                blocks: vec![
                    Block {
                        params: vec![],
                        instructions: vec![],
                        terminator: Terminator::ReplyNew {
                            success: BlockId(1),
                            failure: BlockId(2),
                        },
                    },
                    Block {
                        params: vec![ValueId(0), ValueId(1)],
                        instructions: vec![],
                        terminator: Terminator::Spawn {
                            child: core::FunctionId(1),
                            args: vec![ValueId(0)],
                            success: BlockId(3),
                            failure: BlockId(4),
                            span: Span { start: 0, end: 0 },
                        },
                    },
                    Block {
                        params: vec![ValueId(2)],
                        instructions: vec![instruction(Some(4), Operation::Const(0))],
                        terminator: Terminator::Return(ValueId(4)),
                    },
                    Block {
                        params: vec![],
                        instructions: vec![],
                        terminator: Terminator::ReplyReceive {
                            receiver: ValueId(1),
                            value: BlockId(5),
                            end: BlockId(6),
                        },
                    },
                    Block {
                        params: vec![ValueId(3)],
                        instructions: vec![
                            instruction(None, Operation::CloseReply(ValueId(0))),
                            instruction(None, Operation::CloseReply(ValueId(1))),
                            instruction(Some(5), Operation::Const(0)),
                        ],
                        terminator: Terminator::Return(ValueId(5)),
                    },
                    Block {
                        params: vec![ValueId(6)],
                        instructions: vec![],
                        terminator: Terminator::Return(ValueId(6)),
                    },
                    Block {
                        params: vec![],
                        instructions: vec![instruction(Some(7), Operation::Const(0))],
                        terminator: Terminator::Return(ValueId(7)),
                    },
                ],
            },
            Function {
                name: "reply_worker".into(),
                param_modes: vec![phoebe::syntax::ParamMode::Own],
                return_type: Type::I64,
                value_types: vec![Type::ReplyToI64, Type::I64, Type::I64, Type::I64],
                place_types: vec![],
                loan_regions: vec![],
                blocks: vec![
                    Block {
                        params: vec![ValueId(0)],
                        instructions: vec![instruction(Some(1), Operation::Const(42))],
                        terminator: Terminator::ReplyComplete {
                            right: ValueId(0),
                            value: ValueId(1),
                            delivered: BlockId(1),
                            closed: BlockId(2),
                        },
                    },
                    Block {
                        params: vec![],
                        instructions: vec![instruction(Some(2), Operation::Const(0))],
                        terminator: Terminator::Return(ValueId(2)),
                    },
                    Block {
                        params: vec![],
                        instructions: vec![instruction(Some(3), Operation::Const(0))],
                        terminator: Terminator::Return(ValueId(3)),
                    },
                ],
            },
        ],
    }
}

#[test]
fn checked_core_spawn_moves_reply_right_and_retries_receive() {
    let program = spawned_reply();
    core::verify(&program).unwrap();
    assert_eq!(
        program
            .interpret_with_limits("main", &[], limits(1))
            .unwrap(),
        42
    );
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    tasks: 1,
                    ..limits(1)
                }
            )
            .unwrap(),
        0
    );
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_eq!(
        native::run_with_limits(
            &program,
            "main",
            &[],
            ScopeLimits {
                tasks: 1,
                ..limits(1)
            }
        )
        .unwrap(),
        0
    );
    assert_standalone(&program, "spawn", 42);
}

#[test]
fn checked_core_spawn_moves_right_nested_in_request() {
    let mut program = spawned_reply();
    let parent = &mut program.functions[0];
    parent.value_types.extend([Type::I64, Type::RequestI64]);
    parent.blocks[1].instructions.extend([
        instruction(Some(8), Operation::Const(42)),
        instruction(
            Some(9),
            Operation::PackRequest {
                value: ValueId(8),
                right: ValueId(0),
            },
        ),
    ]);
    if let Terminator::Spawn { args, .. } = &mut parent.blocks[1].terminator {
        *args = vec![ValueId(9)];
    } else {
        unreachable!();
    }
    parent.blocks[4].instructions[0].operation = Operation::CloseRequest(ValueId(9));

    let child = &mut program.functions[1];
    child.value_types[0] = Type::RequestI64;
    child.value_types.push(Type::ReplyToI64);
    child.blocks[0].instructions.clear();
    child.blocks[0].terminator = Terminator::RequestUnpack {
        request: ValueId(0),
        next: BlockId(3),
    };
    child.blocks.push(Block {
        params: vec![ValueId(1), ValueId(4)],
        instructions: vec![],
        terminator: Terminator::ReplyComplete {
            right: ValueId(4),
            value: ValueId(1),
            delivered: BlockId(1),
            closed: BlockId(2),
        },
    });

    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_standalone(&program, "spawn-request", 42);
    let task_limit = ScopeLimits {
        tasks: 1,
        ..limits(1)
    };
    assert_eq!(
        program
            .interpret_with_limits("main", &[], task_limit)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], task_limit).unwrap(),
        0
    );
}
