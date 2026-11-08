use super::*;

fn instruction(result: Option<ValueId>, operation: Operation) -> Instruction {
    Instruction {
        result,
        operation,
        span: Span { start: 0, end: 0 },
    }
}

#[test]
fn nominal_message_crosses_checked_typed_channel_and_returns_to_caller() {
    let data = vec![DataDef {
        name: "Number".into(),
        variants: vec![],
        payloads: vec![],
        fields: vec![("value".into(), Type::I64)],
    }];
    let sender = Type::TypedSender(MessageType::Data(DataId(0)));
    let receiver = Type::TypedReceiver(MessageType::Data(DataId(0)));
    let function = Function {
        name: "main".into(),
        param_modes: vec![],
        return_type: Type::I64,
        value_types: vec![
            Type::I64,             // v0 capacity
            sender,                // v1
            receiver,              // v2
            Type::I64,             // v3 payload
            Type::Data(DataId(0)), // v4 sent value
            Type::Data(DataId(0)), // v5 closed value
            Type::Data(DataId(0)), // v6 received value
            Type::I64,             // v7 failure result
            Type::I64,             // v8 end result
            Type::I64,             // v9 closed result
            Type::I64,             // v10 received field
            Type::ResourceError,   // v11 failure reason
        ],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![
            Block {
                params: vec![],
                instructions: vec![instruction(Some(ValueId(0)), Operation::Const(1))],
                terminator: Terminator::TypedChannelNew {
                    message: MessageType::Data(DataId(0)),
                    capacity: ValueId(0),
                    success: BlockId(1),
                    failure: BlockId(5),
                },
            },
            Block {
                params: vec![ValueId(1), ValueId(2)],
                instructions: vec![
                    instruction(Some(ValueId(3)), Operation::Const(42)),
                    instruction(
                        Some(ValueId(4)),
                        Operation::Product(DataId(0), vec![ValueId(3)]),
                    ),
                ],
                terminator: Terminator::TypedSend {
                    sender: ValueId(1),
                    message: ValueId(4),
                    sent: BlockId(2),
                    closed: BlockId(3),
                },
            },
            Block {
                params: vec![],
                instructions: vec![],
                terminator: Terminator::TypedReceive {
                    receiver: ValueId(2),
                    value: BlockId(4),
                    end: BlockId(6),
                },
            },
            Block {
                params: vec![ValueId(5)],
                instructions: vec![
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseHandle(ValueId(2))),
                    instruction(Some(ValueId(9)), Operation::Const(-2)),
                ],
                terminator: Terminator::Return(ValueId(9)),
            },
            Block {
                params: vec![ValueId(6)],
                instructions: vec![
                    instruction(Some(ValueId(10)), Operation::Field(ValueId(6), 0)),
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseHandle(ValueId(2))),
                ],
                terminator: Terminator::Return(ValueId(10)),
            },
            Block {
                params: vec![ValueId(11)],
                instructions: vec![instruction(Some(ValueId(7)), Operation::Const(-1))],
                terminator: Terminator::Return(ValueId(7)),
            },
            Block {
                params: vec![],
                instructions: vec![
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseHandle(ValueId(2))),
                    instruction(Some(ValueId(8)), Operation::Const(0)),
                ],
                terminator: Terminator::Return(ValueId(8)),
            },
        ],
    };
    let program = Program {
        data,
        functions: vec![function],
    };
    let mut wrong_type = program.clone();
    wrong_type.functions[0].value_types[4] = Type::Bool;
    assert!(CheckedProgram::check(wrong_type).is_err());
    let mut reused_after_send = program.clone();
    reused_after_send.functions[0].value_types.push(Type::I64);
    reused_after_send.functions[0].blocks[2]
        .instructions
        .push(instruction(
            Some(ValueId(12)),
            Operation::Field(ValueId(4), 0),
        ));
    assert!(
        CheckedProgram::check(reused_after_send)
            .err()
            .expect("sent nominal value must be consumed")
            .contains("use after move")
    );
    let checked = CheckedProgram::check(program).unwrap();
    assert_eq!(checked.interpret("main", &[]).unwrap(), 42);
}

#[test]
fn typed_core_moves_reply_right_inside_nominal_message() {
    let data = vec![DataDef {
        name: "Envelope".into(),
        variants: vec![],
        payloads: vec![],
        fields: vec![
            ("value".into(), Type::I64),
            ("reply".into(), Type::ReplyToI64),
        ],
    }];
    let message = MessageType::Data(DataId(0));
    let sender = Type::TypedSender(message);
    let receiver = Type::TypedReceiver(message);
    let product = Type::Data(DataId(0));
    let function = Function {
        name: "main".into(),
        param_modes: vec![],
        return_type: Type::I64,
        value_types: vec![
            Type::I64,              // 0 capacity
            sender,                 // 1
            receiver,               // 2
            Type::ReplyToI64,       // 3
            Type::ReplyReceiverI64, // 4
            Type::I64,              // 5 payload
            product,                // 6 sent product
            product,                // 7 closed product
            product,                // 8 received product
            Type::I64,              // 9 unpacked value
            Type::ReplyToI64,       // 10 unpacked right
            Type::ResourceError,    // 11 channel failure
            Type::ResourceError,    // 12 reply failure
            Type::I64,              // 13 failure return
            Type::I64,              // 14 closed return
            Type::I64,              // 15 reply failure return
            Type::I64,              // 16 end return
        ],
        place_types: vec![],
        loan_regions: vec![],
        blocks: vec![
            Block {
                params: vec![],
                instructions: vec![instruction(Some(ValueId(0)), Operation::Const(1))],
                terminator: Terminator::TypedChannelNew {
                    message,
                    capacity: ValueId(0),
                    success: BlockId(1),
                    failure: BlockId(7),
                },
            },
            Block {
                params: vec![ValueId(1), ValueId(2)],
                instructions: vec![],
                terminator: Terminator::ReplyNew {
                    success: BlockId(2),
                    failure: BlockId(8),
                },
            },
            Block {
                params: vec![ValueId(3), ValueId(4)],
                instructions: vec![
                    instruction(Some(ValueId(5)), Operation::Const(42)),
                    instruction(
                        Some(ValueId(6)),
                        Operation::Product(DataId(0), vec![ValueId(5), ValueId(3)]),
                    ),
                ],
                terminator: Terminator::TypedSend {
                    sender: ValueId(1),
                    message: ValueId(6),
                    sent: BlockId(3),
                    closed: BlockId(4),
                },
            },
            Block {
                params: vec![],
                instructions: vec![],
                terminator: Terminator::TypedReceive {
                    receiver: ValueId(2),
                    value: BlockId(5),
                    end: BlockId(6),
                },
            },
            Block {
                params: vec![ValueId(7)],
                instructions: vec![
                    instruction(None, Operation::CloseCaps(ValueId(7))),
                    instruction(None, Operation::CloseReply(ValueId(4))),
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseHandle(ValueId(2))),
                    instruction(Some(ValueId(14)), Operation::Const(-2)),
                ],
                terminator: Terminator::Return(ValueId(14)),
            },
            Block {
                params: vec![ValueId(8)],
                instructions: vec![],
                terminator: Terminator::ProductUnpack {
                    product: ValueId(8),
                    next: BlockId(9),
                },
            },
            Block {
                params: vec![],
                instructions: vec![
                    instruction(None, Operation::CloseReply(ValueId(4))),
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseHandle(ValueId(2))),
                    instruction(Some(ValueId(16)), Operation::Const(0)),
                ],
                terminator: Terminator::Return(ValueId(16)),
            },
            Block {
                params: vec![ValueId(11)],
                instructions: vec![instruction(Some(ValueId(13)), Operation::Const(-1))],
                terminator: Terminator::Return(ValueId(13)),
            },
            Block {
                params: vec![ValueId(12)],
                instructions: vec![
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseHandle(ValueId(2))),
                    instruction(Some(ValueId(15)), Operation::Const(-3)),
                ],
                terminator: Terminator::Return(ValueId(15)),
            },
            Block {
                params: vec![ValueId(9), ValueId(10)],
                instructions: vec![
                    instruction(None, Operation::CloseReply(ValueId(10))),
                    instruction(None, Operation::CloseReply(ValueId(4))),
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseHandle(ValueId(2))),
                ],
                terminator: Terminator::Return(ValueId(9)),
            },
        ],
    };
    let checked = CheckedProgram::check(Program {
        data,
        functions: vec![function],
    })
    .unwrap();
    assert_eq!(checked.interpret("main", &[]).unwrap(), 42);
}
