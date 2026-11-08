//! Exact active-loan flow for explicit Core borrow.begin / borrow.end pairs.
//! Call-scoped loans remain atomic Call operations and need no active state.

use super::*;

fn successors(terminator: &Terminator) -> Vec<BlockId> {
    match terminator {
        Terminator::Return(_) => vec![],
        Terminator::Yield { resume, .. } | Terminator::ScopeJoin { resume, .. } => vec![*resume],
        Terminator::Jump(target, _)
        | Terminator::RequestUnpack { next: target, .. }
        | Terminator::ProductUnpack { next: target, .. } => vec![*target],
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Match { arms, .. } => arms.iter().map(|arm| arm.target).collect(),
        Terminator::ChannelNew {
            success, failure, ..
        }
        | Terminator::ReplyNew { success, failure }
        | Terminator::RequestChannelNew {
            success, failure, ..
        }
        | Terminator::Fork {
            success, failure, ..
        }
        | Terminator::Spawn {
            success, failure, ..
        } => vec![*success, *failure],
        Terminator::RequestSend { sent, closed, .. }
        | Terminator::ChannelSend { sent, closed, .. } => vec![*sent, *closed],
        Terminator::RequestReceive { value, end, .. }
        | Terminator::ChannelReceive { value, end, .. }
        | Terminator::ReplyReceive { value, end, .. } => vec![*value, *end],
        Terminator::ReplyComplete {
            delivered, closed, ..
        } => vec![*delivered, *closed],
    }
}

fn conflict(
    active: &[bool],
    loans: &[Option<(PlaceId, bool)>],
    root: PlaceId,
    exclusive: bool,
) -> bool {
    active.iter().enumerate().any(|(id, live)| {
        *live && loans[id].is_some_and(|(place, mode)| place == root && (exclusive || mode))
    })
}

pub(super) fn verify_lexical_loans(program: &Program) -> Result<(), String> {
    for function in &program.functions {
        if !function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction.operation,
                    Operation::BorrowBegin(_, _) | Operation::BorrowEnd(_)
                )
            })
        }) {
            continue;
        }
        let mut loans = vec![None; function.value_types.len()];
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Operation::BorrowBegin(place, exclusive) = instruction.operation {
                    let id = instruction.result.ok_or("borrow begin lacks a result")?;
                    loans[id.0] = Some((place, exclusive));
                }
            }
        }
        let mut incoming = vec![None; function.blocks.len()];
        incoming[0] = Some(vec![false; loans.len()]);
        let mut ready = VecDeque::from([0usize]);
        while let Some(block_id) = ready.pop_front() {
            let block = &function.blocks[block_id];
            let mut active = incoming[block_id]
                .clone()
                .expect("queued block has loan state");
            for instruction in &block.instructions {
                for operand in operands(&instruction.operation) {
                    if loans[operand.0].is_some() && !active[operand.0] {
                        return Err(format!("{}: use after borrow end", function.name));
                    }
                }
                match &instruction.operation {
                    Operation::BorrowBegin(place, exclusive) => {
                        if conflict(&active, &loans, *place, *exclusive) {
                            return Err(format!("{}: overlapping lexical borrows", function.name));
                        }
                        active[instruction.result.expect("typed borrow begin").0] = true;
                    }
                    Operation::BorrowEnd(reference) => {
                        if loans[reference.0].is_none() {
                            return Err(format!(
                                "{}: borrow end requires a lexical loan",
                                function.name
                            ));
                        }
                        active[reference.0] = false;
                    }
                    Operation::Load(place)
                    | Operation::ReadField(place, _)
                    | Operation::CopyPlace(place)
                        if conflict(&active, &loans, *place, false) =>
                    {
                        return Err(format!(
                            "{}: read conflicts with exclusive loan",
                            function.name
                        ));
                    }
                    Operation::PlaceInit(place, _)
                    | Operation::Store(place, _)
                    | Operation::Take(place)
                        if conflict(&active, &loans, *place, true) =>
                    {
                        return Err(format!(
                            "{}: mutation conflicts with lexical loan",
                            function.name
                        ));
                    }
                    Operation::Call(_, _) | Operation::GenericCall { .. }
                        if active.iter().any(|live| *live) =>
                    {
                        // A callee may park. Until frame stability is proved for
                        // lexical loans, keep calls outside their live interval.
                        return Err(format!(
                            "{}: call inside lexical loan is not yet supported",
                            function.name
                        ));
                    }
                    _ => {}
                }
            }
            if active.iter().any(|live| *live)
                && matches!(
                    block.terminator,
                    Terminator::Return(_)
                        | Terminator::Yield { .. }
                        | Terminator::ScopeJoin { .. }
                        | Terminator::ChannelSend { .. }
                        | Terminator::ChannelReceive { .. }
                        | Terminator::RequestSend { .. }
                        | Terminator::RequestReceive { .. }
                        | Terminator::ReplyReceive { .. }
                        | Terminator::Spawn { .. }
                )
            {
                return Err(format!(
                    "{}: lexical loan crosses exit or suspension",
                    function.name
                ));
            }
            for target in successors(&block.terminator) {
                match &incoming[target.0] {
                    None => {
                        incoming[target.0] = Some(active.clone());
                        ready.push_back(target.0);
                    }
                    Some(existing) if existing != &active => {
                        return Err(format!(
                            "{}: lexical loan states differ at a CFG join",
                            function.name
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
