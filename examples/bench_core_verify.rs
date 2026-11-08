use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, Function, Instruction, Operation, Program, Terminator, Type, ValueId,
};
use std::time::Instant;

fn chain(count: usize) -> Program {
    let mut blocks = Vec::with_capacity(count);
    blocks.push(Block {
        params: vec![],
        instructions: vec![Instruction {
            result: Some(ValueId(0)),
            operation: Operation::Const(42),
            span: Span { start: 0, end: 0 },
        }],
        terminator: if count == 1 {
            Terminator::Return(ValueId(0))
        } else {
            Terminator::Jump(BlockId(1), vec![])
        },
    });
    for id in 1..count {
        blocks.push(Block {
            params: vec![],
            instructions: vec![],
            terminator: if id + 1 == count {
                Terminator::Return(ValueId(0))
            } else {
                Terminator::Jump(BlockId(id + 1), vec![])
            },
        });
    }
    Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            blocks,
            value_types: vec![Type::I64],
            place_types: vec![],
            loan_regions: vec![],
        }],
    }
}

fn main() {
    for count in [100, 500, 1000, 2000] {
        let program = chain(count);
        core::verify(&program).unwrap();
        let mut samples = Vec::new();
        for _ in 0..7 {
            let start = Instant::now();
            core::verify(&program).unwrap();
            samples.push(start.elapsed().as_micros());
        }
        samples.sort();
        println!("{count} blocks: {} us", samples[3]);
    }
}
