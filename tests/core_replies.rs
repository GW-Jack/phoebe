use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, Function, Instruction, Operation, Program, Terminator, Type, ValueId,
};
use phoebe::native;
use phoebe::runtime::ScopeLimits;

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
    let error = native::run(&program, "main", &[]).unwrap_err();
    assert!(error.contains("reply"), "{error}");
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
}
