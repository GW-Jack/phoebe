use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, CallArg, Function, FunctionId, Instruction, Operation, Program,
    Terminator, Type, ValueId,
};
use phoebe::native;
use phoebe::runtime::ScopeLimits;
use phoebe::syntax::ParamMode;
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

fn channel_program() -> Program {
    Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![
                Type::I64,
                Type::SenderI64,
                Type::ReceiverI64,
                Type::ResourceError,
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
                    instructions: vec![
                        instruction(None, Operation::CloseHandle(ValueId(1))),
                        instruction(None, Operation::CloseHandle(ValueId(2))),
                        instruction(Some(4), Operation::Const(7)),
                    ],
                    terminator: Terminator::Return(ValueId(4)),
                },
                Block {
                    params: vec![ValueId(3)],
                    instructions: vec![instruction(Some(5), Operation::Const(0))],
                    terminator: Terminator::Return(ValueId(5)),
                },
            ],
        }],
    }
}

fn send_receive_program() -> Program {
    let mut program = channel_program();
    let function = &mut program.functions[0];
    function.value_types.extend([Type::I64; 5]);
    function.blocks[1].instructions = vec![instruction(Some(4), Operation::Const(41))];
    function.blocks[1].terminator = Terminator::ChannelSend {
        sender: ValueId(1),
        message: ValueId(4),
        sent: BlockId(3),
        closed: BlockId(5),
    };
    function.blocks.push(Block {
        params: vec![],
        instructions: vec![],
        terminator: Terminator::ChannelReceive {
            receiver: ValueId(2),
            value: BlockId(4),
            end: BlockId(6),
        },
    });
    function.blocks.push(Block {
        params: vec![ValueId(6)],
        instructions: vec![
            instruction(None, Operation::CloseHandle(ValueId(1))),
            instruction(None, Operation::CloseHandle(ValueId(2))),
            instruction(Some(7), Operation::Const(1)),
            instruction(Some(8), Operation::Add(ValueId(6), ValueId(7))),
        ],
        terminator: Terminator::Return(ValueId(8)),
    });
    for (block_id, result) in [(5, 9), (6, 10)] {
        function.blocks.push(Block {
            params: vec![],
            instructions: vec![
                instruction(None, Operation::CloseHandle(ValueId(1))),
                instruction(None, Operation::CloseHandle(ValueId(2))),
                instruction(Some(result), Operation::Const(block_id as i64)),
            ],
            terminator: Terminator::Return(ValueId(result)),
        });
    }
    program
}

#[test]
fn checked_core_send_and_receive_preserve_rights_and_value() {
    let program = send_receive_program();
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    let printed = program.to_string();
    assert!(printed.contains("channel.send.i64"));
    assert!(printed.contains("channel.receive.i64"));
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn core_receive_parks_with_a_retained_frame_until_scope_deadlock() {
    let mut program = channel_program();
    let function = &mut program.functions[0];
    function.value_types.push(Type::I64);
    function.blocks[1].instructions.clear();
    function.blocks[1].terminator = Terminator::ChannelReceive {
        receiver: ValueId(2),
        value: BlockId(3),
        end: BlockId(4),
    };
    function.blocks.push(Block {
        params: vec![ValueId(6)],
        instructions: vec![
            instruction(None, Operation::CloseHandle(ValueId(1))),
            instruction(None, Operation::CloseHandle(ValueId(2))),
        ],
        terminator: Terminator::Return(ValueId(6)),
    });
    function.blocks.push(Block {
        params: vec![],
        instructions: vec![
            instruction(None, Operation::CloseHandle(ValueId(1))),
            instruction(None, Operation::CloseHandle(ValueId(2))),
            instruction(Some(4), Operation::Const(0)),
        ],
        terminator: Terminator::Return(ValueId(4)),
    });
    core::verify(&program).unwrap();
    assert!(
        program
            .interpret("main", &[])
            .unwrap_err()
            .contains("scope deadlocked")
    );
}

#[test]
fn owned_endpoint_call_transfers_the_right_to_the_callee() {
    let mut program = channel_program();
    let main = &mut program.functions[0];
    main.blocks[1].instructions = vec![
        instruction(None, Operation::CloseHandle(ValueId(2))),
        instruction(
            Some(4),
            Operation::Call(FunctionId(1), vec![CallArg::Value(ValueId(1))]),
        ),
    ];
    main.blocks[1].terminator = Terminator::Return(ValueId(4));
    program.functions.push(Function {
        name: "close_sender".into(),
        param_modes: vec![ParamMode::Own],
        return_type: Type::I64,
        value_types: vec![Type::SenderI64, Type::I64],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![Block {
            params: vec![ValueId(0)],
            instructions: vec![
                instruction(None, Operation::CloseHandle(ValueId(0))),
                instruction(Some(1), Operation::Const(9)),
            ],
            terminator: Terminator::Return(ValueId(1)),
        }],
    });
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 9);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 9);

    let main = &mut program.functions[0];
    main.blocks[1]
        .instructions
        .push(instruction(None, Operation::CloseHandle(ValueId(1))));
    assert!(core::verify(&program).unwrap_err().contains("is not live"));
}

#[test]
fn checked_channel_creation_has_two_success_rights_and_a_typed_failure() {
    let program = channel_program();
    core::verify(&program).unwrap();
    let printed = program.to_string();
    assert!(printed.contains("channel.new.i64 v0 success block1 failure block2"));
    assert!(printed.contains("close.handle v1"));
    assert!(native::clif(&program).unwrap().contains("call"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
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
                },
            )
            .unwrap(),
        0
    );
    let mut zero_capacity = program.clone();
    zero_capacity.functions[0].blocks[0].instructions[0].operation = Operation::Const(0);
    assert_eq!(zero_capacity.interpret("main", &[]).unwrap(), 0);
    let jit = native::compile_jit(&program, "main").unwrap();
    assert_eq!(jit.run(&[]).unwrap(), 7);
    assert_eq!(
        jit.run_with_limits(
            &[],
            ScopeLimits {
                tasks: 1,
                channels: 0,
                replies: 0,
                ..ScopeLimits::default()
            },
        )
        .unwrap(),
        0
    );
    assert_eq!(native::run(&zero_capacity, "main", &[]).unwrap(), 0);
    assert!(!native::emit_object(&program).unwrap().is_empty());
    let path = std::env::temp_dir().join(format!("phoebe-channel-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"7\n");

    let failure_path =
        std::env::temp_dir().join(format!("phoebe-channel-failure-aot-{}", std::process::id()));
    native::build_executable(&zero_capacity, &failure_path).unwrap();
    let failure = Command::new(&failure_path).output().unwrap();
    std::fs::remove_file(&failure_path).unwrap();
    assert!(
        failure.status.success(),
        "{}",
        String::from_utf8_lossy(&failure.stderr)
    );
    assert_eq!(failure.stdout, b"0\n");
}

#[test]
fn channel_creation_rejects_a_forgotten_right() {
    let mut program = channel_program();
    program.functions[0].blocks[1].instructions.remove(1);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("normal return leaks")
    );

    let mut program = channel_program();
    program.functions[0].blocks[1]
        .instructions
        .insert(1, instruction(None, Operation::CloseHandle(ValueId(1))));
    assert!(core::verify(&program).unwrap_err().contains("is not live"));
}

#[test]
fn channel_rights_move_through_typed_block_arguments() {
    let mut program = channel_program();
    let function = &mut program.functions[0];
    function.blocks[1].instructions.drain(0..2);
    function.blocks[1].terminator = Terminator::Jump(BlockId(3), vec![ValueId(1), ValueId(2)]);
    function
        .value_types
        .extend([Type::SenderI64, Type::ReceiverI64, Type::I64]);
    function.blocks.push(Block {
        params: vec![ValueId(6), ValueId(7)],
        instructions: vec![
            instruction(None, Operation::CloseHandle(ValueId(6))),
            instruction(None, Operation::CloseHandle(ValueId(7))),
            instruction(Some(8), Operation::Const(9)),
        ],
        terminator: Terminator::Return(ValueId(8)),
    });
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 9);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 9);
    program.functions[0].blocks[3].instructions.remove(1);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("normal return leaks")
    );
}

#[test]
fn channel_creation_checks_edge_signatures_and_distinctness() {
    let mut program = channel_program();
    program.functions[0].value_types[1] = Type::ReceiverI64;
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("edge signature mismatch")
    );

    let mut program = channel_program();
    if let Terminator::ChannelNew { failure, .. } = &mut program.functions[0].blocks[0].terminator {
        *failure = BlockId(1);
    }
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("creation edges must be distinct")
    );

    let mut program = channel_program();
    program.functions[0].value_types[0] = Type::Bool;
    program.functions[0].blocks[0].instructions[0].operation = Operation::Bool(true);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("capacity must be I64")
    );
}

#[test]
fn nested_call_receives_the_native_runtime_context() {
    let mut program = channel_program();
    let main = &mut program.functions[0];
    main.value_types.push(Type::I64);
    main.blocks[1].instructions.push(instruction(
        Some(6),
        Operation::Call(FunctionId(1), vec![CallArg::Value(ValueId(4))]),
    ));
    main.blocks[1].terminator = Terminator::Return(ValueId(6));
    program.functions.push(Function {
        name: "bump".into(),
        param_modes: vec![ParamMode::Own],
        return_type: Type::I64,
        value_types: vec![Type::I64, Type::I64, Type::I64],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![Block {
            params: vec![ValueId(0)],
            instructions: vec![
                instruction(Some(1), Operation::Const(1)),
                instruction(Some(2), Operation::Add(ValueId(0), ValueId(1))),
            ],
            terminator: Terminator::Return(ValueId(2)),
        }],
    });
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 8);
    let main = native::compile_jit(&program, "main").unwrap();
    assert!(main.uses_runtime_context());
    assert_eq!(main.run(&[]).unwrap(), 8);
    let bump = native::compile_jit(&program, "bump").unwrap();
    assert!(!bump.uses_runtime_context());
    assert_eq!(bump.run(&[41]).unwrap(), 42);
}

#[test]
fn runtime_context_propagates_to_callers_only() {
    let mut program = channel_program();
    program.functions[0].name = "make_and_close".into();
    program.functions.push(Function {
        name: "main".into(),
        param_modes: vec![],
        return_type: Type::I64,
        value_types: vec![Type::I64],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![Block {
            params: vec![],
            instructions: vec![instruction(Some(0), Operation::Call(FunctionId(0), vec![]))],
            terminator: Terminator::Return(ValueId(0)),
        }],
    });
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 7);
    let jit = native::compile_jit(&program, "main").unwrap();
    assert!(jit.uses_runtime_context());
    assert_eq!(jit.run(&[]).unwrap(), 7);
    let path = std::env::temp_dir().join(format!("phoebe-channel-call-aot-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let result = Command::new(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"7\n");
}
