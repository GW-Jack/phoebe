use phoebe::{core, frame_layout, native, syntax};
use std::process::Command;

#[test]
fn live_loan_crosses_channel_send() {
    let source = include_str!("../examples/channel_exchange.phoebe").replace(
        "val first = sender.send(20);",
        "var x = 41; val first = sender.send(20);",
    );
    let mut program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    let producer = program
        .functions
        .iter_mut()
        .find(|function| function.name == "producer")
        .unwrap();
    assert_eq!(producer.place_types[0], core::Type::I64);
    let (send_block, sent_block, closed_block) = producer
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(id, block)| match block.terminator {
            core::Terminator::ChannelSend { sent, closed, .. } => Some((id, sent, closed)),
            _ => None,
        })
        .nth(1)
        .unwrap();
    let reference = core::ValueId(producer.value_types.len());
    let sent_read = core::ValueId(reference.0 + 1);
    let closed_read = core::ValueId(reference.0 + 2);
    producer
        .value_types
        .extend([core::Type::RefI64, core::Type::I64, core::Type::I64]);
    let span = producer.blocks[send_block].instructions[0].span;
    producer.blocks[send_block]
        .instructions
        .push(core::Instruction {
            result: Some(reference),
            operation: core::Operation::BorrowBegin(core::PlaceId(0), false),
            span,
        });
    for (target, read) in [(sent_block, sent_read), (closed_block, closed_read)] {
        producer.blocks[target.0].instructions.splice(
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
    let producer_frame = frames
        .iter()
        .find(|frame| program.functions[frame.function.0].name == "producer")
        .unwrap();
    let point = producer_frame
        .points
        .iter()
        .find(|point| point.block.0 == send_block)
        .unwrap();
    assert!(point.values.contains(&reference));
    assert!(point.places.contains(&core::PlaceId(0)));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-live-send-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}
