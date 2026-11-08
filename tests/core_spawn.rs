use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, Function, FunctionId, Instruction, Operation, Program, Terminator, Type,
    ValueId,
};
use phoebe::native;
use phoebe::runtime::ScopeLimits;
use phoebe::syntax::ParamMode;

fn instruction(result: Option<usize>, operation: Operation) -> Instruction {
    Instruction {
        result: result.map(ValueId),
        operation,
        span: Span { start: 0, end: 0 },
    }
}

fn two_task_program() -> Program {
    Program {
        data: vec![],
        functions: vec![
            Function {
                name: "main".into(),
                param_modes: vec![],
                return_type: Type::I64,
                value_types: vec![
                    Type::I64,
                    Type::SenderI64,
                    Type::ReceiverI64,
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
                        instructions: vec![instruction(Some(0), Operation::Const(1))],
                        terminator: Terminator::ChannelNew {
                            capacity: ValueId(0),
                            success: BlockId(1),
                            failure: BlockId(2),
                        },
                    },
                    Block {
                        params: vec![ValueId(1), ValueId(2)],
                        instructions: vec![],
                        terminator: Terminator::Spawn {
                            child: FunctionId(1),
                            args: vec![ValueId(1)],
                            success: BlockId(3),
                            failure: BlockId(4),
                            span: Span { start: 0, end: 0 },
                        },
                    },
                    Block {
                        params: vec![ValueId(3)],
                        instructions: vec![instruction(Some(6), Operation::Const(0))],
                        terminator: Terminator::Return(ValueId(6)),
                    },
                    Block {
                        params: vec![],
                        instructions: vec![],
                        terminator: Terminator::ChannelReceive {
                            receiver: ValueId(2),
                            value: BlockId(5),
                            end: BlockId(6),
                        },
                    },
                    Block {
                        params: vec![ValueId(4)],
                        instructions: vec![
                            instruction(None, Operation::CloseHandle(ValueId(1))),
                            instruction(None, Operation::CloseHandle(ValueId(2))),
                            instruction(Some(7), Operation::Const(0)),
                        ],
                        terminator: Terminator::Return(ValueId(7)),
                    },
                    Block {
                        params: vec![ValueId(5)],
                        instructions: vec![instruction(None, Operation::CloseHandle(ValueId(2)))],
                        terminator: Terminator::Return(ValueId(5)),
                    },
                    Block {
                        params: vec![],
                        instructions: vec![
                            instruction(None, Operation::CloseHandle(ValueId(2))),
                            instruction(Some(8), Operation::Const(0)),
                        ],
                        terminator: Terminator::Return(ValueId(8)),
                    },
                ],
            },
            Function {
                name: "producer".into(),
                param_modes: vec![ParamMode::Own],
                return_type: Type::I64,
                value_types: vec![Type::SenderI64, Type::I64, Type::I64, Type::I64],
                place_types: vec![],
                loan_regions: vec![],
                blocks: vec![
                    Block {
                        params: vec![ValueId(0)],
                        instructions: vec![instruction(Some(1), Operation::Const(42))],
                        terminator: Terminator::ChannelSend {
                            sender: ValueId(0),
                            message: ValueId(1),
                            sent: BlockId(1),
                            closed: BlockId(2),
                        },
                    },
                    Block {
                        params: vec![],
                        instructions: vec![
                            instruction(None, Operation::CloseHandle(ValueId(0))),
                            instruction(Some(2), Operation::Const(0)),
                        ],
                        terminator: Terminator::Return(ValueId(2)),
                    },
                    Block {
                        params: vec![],
                        instructions: vec![
                            instruction(None, Operation::CloseHandle(ValueId(0))),
                            instruction(Some(3), Operation::Const(0)),
                        ],
                        terminator: Terminator::Return(ValueId(3)),
                    },
                ],
            },
        ],
    }
}

#[test]
fn checked_core_spawn_transfers_sender_and_wakes_parent_receiver() {
    let program = two_task_program();
    core::verify(&program).unwrap();
    assert!(program.to_string().contains("spawn producer(v1)"));
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    tasks: 2,
                    channels: 1,
                    replies: 0,
                    ..ScopeLimits::default()
                }
            )
            .unwrap(),
        42
    );
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn failed_spawn_keeps_both_endpoints_with_parent() {
    let program = two_task_program();
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    tasks: 1,
                    channels: 1,
                    replies: 0,
                    ..ScopeLimits::default()
                }
            )
            .unwrap(),
        0
    );
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    tasks: 2,
                    channels: 0,
                    replies: 0,
                    ..ScopeLimits::default()
                }
            )
            .unwrap(),
        0
    );
}

#[test]
fn verifier_rejects_right_use_after_spawn_and_failure_leak() {
    let mut program = two_task_program();
    program.functions[0].blocks[3]
        .instructions
        .push(instruction(None, Operation::CloseHandle(ValueId(1))));
    assert!(core::verify(&program).unwrap_err().contains("is not live"));

    let mut program = two_task_program();
    program.functions[0].blocks[4].instructions.remove(0);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("normal return leaks")
    );
}

#[test]
fn closed_nominal_argument_moves_only_if_spawn_commits() {
    let badge = core::DataId(0);
    let mut program = Program {
        data: vec![core::DataDef {
            name: "Badge".into(),
            variants: vec!["One".into()],
            payloads: vec![None],
            fields: vec![],
        }],
        functions: vec![
            Function {
                name: "main".into(),
                param_modes: vec![],
                return_type: Type::I64,
                value_types: vec![Type::Data(badge), Type::ResourceError, Type::I64, Type::I64],
                place_types: vec![],
                loan_regions: vec![],
                blocks: vec![
                    Block {
                        params: vec![],
                        instructions: vec![instruction(
                            Some(0),
                            Operation::Variant(badge, 0, None),
                        )],
                        terminator: Terminator::Spawn {
                            child: FunctionId(1),
                            args: vec![ValueId(0)],
                            success: BlockId(1),
                            failure: BlockId(2),
                            span: Span { start: 0, end: 0 },
                        },
                    },
                    Block {
                        params: vec![],
                        instructions: vec![instruction(Some(2), Operation::Const(1))],
                        terminator: Terminator::Return(ValueId(2)),
                    },
                    Block {
                        params: vec![ValueId(1)],
                        instructions: vec![instruction(
                            Some(3),
                            Operation::Call(FunctionId(1), vec![core::CallArg::Value(ValueId(0))]),
                        )],
                        terminator: Terminator::Return(ValueId(3)),
                    },
                ],
            },
            Function {
                name: "consume_badge".into(),
                param_modes: vec![ParamMode::Own],
                return_type: Type::I64,
                value_types: vec![Type::Data(badge), Type::I64],
                place_types: vec![],
                loan_regions: vec![],
                blocks: vec![Block {
                    params: vec![ValueId(0)],
                    instructions: vec![instruction(Some(1), Operation::Const(9))],
                    terminator: Terminator::Return(ValueId(1)),
                }],
            },
        ],
    };
    core::verify(&program).unwrap();
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    tasks: 1,
                    channels: 0,
                    replies: 0,
                    ..ScopeLimits::default()
                }
            )
            .unwrap(),
        9
    );
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    tasks: 2,
                    channels: 0,
                    replies: 0,
                    ..ScopeLimits::default()
                }
            )
            .unwrap(),
        1
    );
    program.functions[0].value_types.push(Type::I64);
    program.functions[0].blocks[1].instructions.insert(
        0,
        instruction(
            Some(4),
            Operation::Call(FunctionId(1), vec![core::CallArg::Value(ValueId(0))]),
        ),
    );
    program.functions[0].blocks[1].terminator = Terminator::Return(ValueId(4));
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("use after move")
    );
}
