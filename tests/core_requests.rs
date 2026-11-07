use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, Function, Instruction, Operation, Program, Terminator, Type, ValueId,
};
use phoebe::native;
use phoebe::runtime::ScopeLimits;

fn instruction(result: Option<usize>, operation: Operation) -> Instruction {
    Instruction {
        result: result.map(ValueId),
        operation,
        span: Span { start: 0, end: 0 },
    }
}

fn block(params: &[usize], instructions: Vec<Instruction>, terminator: Terminator) -> Block {
    Block {
        params: params.iter().copied().map(ValueId).collect(),
        instructions,
        terminator,
    }
}

fn request_round_trip() -> Program {
    Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![
                Type::I64,                // 0 capacity
                Type::RequestSenderI64,   // 1 sender
                Type::RequestReceiverI64, // 2 receiver
                Type::ResourceError,      // 3 channel error
                Type::ReplyToI64,         // 4 right
                Type::ReplyReceiverI64,   // 5 reply receiver
                Type::ResourceError,      // 6 reply error
                Type::I64,                // 7 request value
                Type::RequestI64,         // 8 packed request
                Type::RequestI64,         // 9 received request
                Type::I64,                // 10 unpacked value
                Type::ReplyToI64,         // 11 unpacked right
                Type::I64,                // 12 received reply
                Type::I64,                // 13 channel error result
                Type::I64,                // 14 reply error result
                Type::RequestI64,         // 15 rejected request
                Type::I64,                // 16 rejected send result
                Type::I64,                // 17 request end result
                Type::I64,                // 18 completion closed result
                Type::I64,                // 19 reply end result
            ],
            place_types: vec![],
            blocks: vec![
                block(
                    &[],
                    vec![instruction(Some(0), Operation::Const(1))],
                    Terminator::RequestChannelNew {
                        capacity: ValueId(0),
                        success: BlockId(1),
                        failure: BlockId(2),
                    },
                ),
                block(
                    &[1, 2],
                    vec![],
                    Terminator::ReplyNew {
                        success: BlockId(3),
                        failure: BlockId(4),
                    },
                ),
                block(
                    &[3],
                    vec![instruction(Some(13), Operation::Const(0))],
                    Terminator::Return(ValueId(13)),
                ),
                block(
                    &[4, 5],
                    vec![
                        instruction(Some(7), Operation::Const(42)),
                        instruction(
                            Some(8),
                            Operation::PackRequest {
                                value: ValueId(7),
                                right: ValueId(4),
                            },
                        ),
                    ],
                    Terminator::RequestSend {
                        sender: ValueId(1),
                        request: ValueId(8),
                        sent: BlockId(5),
                        closed: BlockId(7),
                    },
                ),
                block(
                    &[6],
                    vec![
                        instruction(None, Operation::CloseHandle(ValueId(1))),
                        instruction(None, Operation::CloseHandle(ValueId(2))),
                        instruction(Some(14), Operation::Const(0)),
                    ],
                    Terminator::Return(ValueId(14)),
                ),
                block(
                    &[],
                    vec![instruction(None, Operation::CloseHandle(ValueId(1)))],
                    Terminator::RequestReceive {
                        receiver: ValueId(2),
                        value: BlockId(6),
                        end: BlockId(9),
                    },
                ),
                block(
                    &[9],
                    vec![],
                    Terminator::RequestUnpack {
                        request: ValueId(9),
                        next: BlockId(8),
                    },
                ),
                block(
                    &[15],
                    vec![
                        instruction(None, Operation::CloseRequest(ValueId(15))),
                        instruction(None, Operation::CloseHandle(ValueId(1))),
                        instruction(None, Operation::CloseHandle(ValueId(2))),
                        instruction(None, Operation::CloseReply(ValueId(5))),
                        instruction(Some(16), Operation::Const(0)),
                    ],
                    Terminator::Return(ValueId(16)),
                ),
                block(
                    &[10, 11],
                    vec![instruction(None, Operation::CloseHandle(ValueId(2)))],
                    Terminator::ReplyComplete {
                        right: ValueId(11),
                        value: ValueId(10),
                        delivered: BlockId(10),
                        closed: BlockId(11),
                    },
                ),
                block(
                    &[],
                    vec![
                        instruction(None, Operation::CloseHandle(ValueId(2))),
                        instruction(None, Operation::CloseReply(ValueId(5))),
                        instruction(Some(17), Operation::Const(0)),
                    ],
                    Terminator::Return(ValueId(17)),
                ),
                block(
                    &[],
                    vec![],
                    Terminator::ReplyReceive {
                        receiver: ValueId(5),
                        value: BlockId(12),
                        end: BlockId(13),
                    },
                ),
                block(
                    &[],
                    vec![
                        instruction(None, Operation::CloseReply(ValueId(5))),
                        instruction(Some(18), Operation::Const(0)),
                    ],
                    Terminator::Return(ValueId(18)),
                ),
                block(&[12], vec![], Terminator::Return(ValueId(12))),
                block(
                    &[],
                    vec![instruction(Some(19), Operation::Const(0))],
                    Terminator::Return(ValueId(19)),
                ),
            ],
        }],
    }
}

#[test]
fn checked_core_moves_reply_through_request_queue() {
    let program = request_round_trip();
    core::verify(&program).unwrap();
    assert!(program.to_string().contains("request.pack"));
    assert!(program.to_string().contains("channel.send.request.i64"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(
        program
            .interpret_with_limits(
                "main",
                &[],
                ScopeLimits {
                    channels: 0,
                    ..ScopeLimits::default()
                },
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
                    replies: 0,
                    ..ScopeLimits::default()
                },
            )
            .unwrap(),
        0
    );
    assert!(
        native::run(&program, "main", &[])
            .unwrap_err()
            .contains("request-message lowering")
    );
}

#[test]
fn request_pack_cannot_leave_the_original_reply_right_live() {
    let mut program = request_round_trip();
    program.functions[0].blocks[3]
        .instructions
        .push(instruction(None, Operation::CloseReply(ValueId(4))));
    let error = core::verify(&program).unwrap_err();
    assert!(error.contains("not live"), "{error}");
}

#[test]
fn rejected_request_must_be_closed_or_unpacked() {
    let mut program = request_round_trip();
    program.functions[0].blocks[7].instructions.remove(0);
    let error = core::verify(&program).unwrap_err();
    assert!(error.contains("leaks capabilities"), "{error}");
}

#[test]
fn closed_request_send_returns_the_intact_owned_request() {
    let program = Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![
                Type::I64,
                Type::RequestSenderI64,
                Type::RequestReceiverI64,
                Type::ResourceError,
                Type::ReplyToI64,
                Type::ReplyReceiverI64,
                Type::ResourceError,
                Type::I64,
                Type::RequestI64,
                Type::RequestI64,
                Type::I64,
                Type::I64,
                Type::I64,
                Type::I64,
            ],
            place_types: vec![],
            blocks: vec![
                block(
                    &[],
                    vec![instruction(Some(0), Operation::Const(1))],
                    Terminator::RequestChannelNew {
                        capacity: ValueId(0),
                        success: BlockId(1),
                        failure: BlockId(2),
                    },
                ),
                block(
                    &[1, 2],
                    vec![],
                    Terminator::ReplyNew {
                        success: BlockId(3),
                        failure: BlockId(4),
                    },
                ),
                block(
                    &[3],
                    vec![instruction(Some(10), Operation::Const(0))],
                    Terminator::Return(ValueId(10)),
                ),
                block(
                    &[4, 5],
                    vec![
                        instruction(None, Operation::CloseHandle(ValueId(2))),
                        instruction(Some(7), Operation::Const(42)),
                        instruction(
                            Some(8),
                            Operation::PackRequest {
                                value: ValueId(7),
                                right: ValueId(4),
                            },
                        ),
                    ],
                    Terminator::RequestSend {
                        sender: ValueId(1),
                        request: ValueId(8),
                        sent: BlockId(5),
                        closed: BlockId(6),
                    },
                ),
                block(
                    &[6],
                    vec![
                        instruction(None, Operation::CloseHandle(ValueId(1))),
                        instruction(None, Operation::CloseHandle(ValueId(2))),
                        instruction(Some(11), Operation::Const(0)),
                    ],
                    Terminator::Return(ValueId(11)),
                ),
                block(
                    &[],
                    vec![
                        instruction(None, Operation::CloseHandle(ValueId(1))),
                        instruction(None, Operation::CloseReply(ValueId(5))),
                        instruction(Some(12), Operation::Const(0)),
                    ],
                    Terminator::Return(ValueId(12)),
                ),
                block(
                    &[9],
                    vec![
                        instruction(None, Operation::CloseRequest(ValueId(9))),
                        instruction(None, Operation::CloseHandle(ValueId(1))),
                        instruction(None, Operation::CloseReply(ValueId(5))),
                        instruction(Some(13), Operation::Const(42)),
                    ],
                    Terminator::Return(ValueId(13)),
                ),
            ],
        }],
    };
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
}

#[test]
fn request_service_parks_then_completes_parent_reply() {
    let main = Function {
        name: "main".into(),
        param_modes: vec![],
        return_type: Type::I64,
        value_types: vec![
            Type::I64,
            Type::RequestSenderI64,
            Type::RequestReceiverI64,
            Type::ResourceError,
            Type::ReplyToI64,
            Type::ReplyReceiverI64,
            Type::ResourceError,
            Type::ResourceError,
            Type::I64,
            Type::RequestI64,
            Type::RequestI64,
            Type::I64,
            Type::I64,
            Type::I64,
            Type::I64,
            Type::I64,
            Type::I64,
        ],
        place_types: vec![],
        blocks: vec![
            block(
                &[],
                vec![instruction(Some(0), Operation::Const(1))],
                Terminator::RequestChannelNew {
                    capacity: ValueId(0),
                    success: BlockId(1),
                    failure: BlockId(2),
                },
            ),
            block(
                &[1, 2],
                vec![],
                Terminator::ReplyNew {
                    success: BlockId(3),
                    failure: BlockId(4),
                },
            ),
            block(
                &[3],
                vec![instruction(Some(12), Operation::Const(0))],
                Terminator::Return(ValueId(12)),
            ),
            block(
                &[4, 5],
                vec![],
                Terminator::Spawn {
                    child: core::FunctionId(1),
                    args: vec![ValueId(2)],
                    success: BlockId(5),
                    failure: BlockId(6),
                },
            ),
            block(
                &[6],
                vec![
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseHandle(ValueId(2))),
                    instruction(Some(13), Operation::Const(0)),
                ],
                Terminator::Return(ValueId(13)),
            ),
            block(
                &[],
                vec![],
                Terminator::Yield {
                    resume: BlockId(11),
                    span: Span { start: 0, end: 0 },
                },
            ),
            block(
                &[7],
                vec![
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseHandle(ValueId(2))),
                    instruction(None, Operation::CloseReply(ValueId(4))),
                    instruction(None, Operation::CloseReply(ValueId(5))),
                    instruction(Some(14), Operation::Const(0)),
                ],
                Terminator::Return(ValueId(14)),
            ),
            block(
                &[],
                vec![instruction(None, Operation::CloseHandle(ValueId(1)))],
                Terminator::ReplyReceive {
                    receiver: ValueId(5),
                    value: BlockId(9),
                    end: BlockId(10),
                },
            ),
            block(
                &[10],
                vec![
                    instruction(None, Operation::CloseRequest(ValueId(10))),
                    instruction(None, Operation::CloseHandle(ValueId(1))),
                    instruction(None, Operation::CloseReply(ValueId(5))),
                    instruction(Some(15), Operation::Const(0)),
                ],
                Terminator::Return(ValueId(15)),
            ),
            block(&[11], vec![], Terminator::Return(ValueId(11))),
            block(
                &[],
                vec![instruction(Some(16), Operation::Const(0))],
                Terminator::Return(ValueId(16)),
            ),
            block(
                &[],
                vec![
                    instruction(Some(8), Operation::Const(42)),
                    instruction(
                        Some(9),
                        Operation::PackRequest {
                            value: ValueId(8),
                            right: ValueId(4),
                        },
                    ),
                ],
                Terminator::RequestSend {
                    sender: ValueId(1),
                    request: ValueId(9),
                    sent: BlockId(7),
                    closed: BlockId(8),
                },
            ),
        ],
    };
    let service = Function {
        name: "service".into(),
        param_modes: vec![phoebe::syntax::syntax::ParamMode::Own],
        return_type: Type::I64,
        value_types: vec![
            Type::RequestReceiverI64,
            Type::RequestI64,
            Type::I64,
            Type::ReplyToI64,
            Type::I64,
            Type::I64,
            Type::I64,
        ],
        place_types: vec![],
        blocks: vec![
            block(
                &[0],
                vec![],
                Terminator::RequestReceive {
                    receiver: ValueId(0),
                    value: BlockId(1),
                    end: BlockId(2),
                },
            ),
            block(
                &[1],
                vec![],
                Terminator::RequestUnpack {
                    request: ValueId(1),
                    next: BlockId(3),
                },
            ),
            block(
                &[],
                vec![
                    instruction(None, Operation::CloseHandle(ValueId(0))),
                    instruction(Some(4), Operation::Const(0)),
                ],
                Terminator::Return(ValueId(4)),
            ),
            block(
                &[2, 3],
                vec![instruction(None, Operation::CloseHandle(ValueId(0)))],
                Terminator::ReplyComplete {
                    right: ValueId(3),
                    value: ValueId(2),
                    delivered: BlockId(4),
                    closed: BlockId(5),
                },
            ),
            block(
                &[],
                vec![instruction(Some(5), Operation::Const(0))],
                Terminator::Return(ValueId(5)),
            ),
            block(
                &[],
                vec![instruction(Some(6), Operation::Const(0))],
                Terminator::Return(ValueId(6)),
            ),
        ],
    };
    let program = Program {
        data: vec![],
        functions: vec![main, service],
    };
    core::verify(&program).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
}
