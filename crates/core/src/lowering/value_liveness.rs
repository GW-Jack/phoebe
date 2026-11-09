//! Backward liveness for choosing a transfer on a closed value's last use.

use super::*;
use std::collections::HashSet;

fn successors(end: &Terminator) -> Vec<BlockId> {
    match end {
        Terminator::Return(_) => vec![],
        Terminator::Yield { resume, .. }
        | Terminator::ScopeJoin { resume, .. }
        | Terminator::RequestUnpack { next: resume, .. }
        | Terminator::ProductUnpack { next: resume, .. }
        | Terminator::Jump(resume, _) => vec![*resume],
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Match { arms, .. } => arms.iter().map(|arm| arm.target).collect(),
        Terminator::ChannelNew {
            success, failure, ..
        }
        | Terminator::TypedChannelNew {
            success, failure, ..
        }
        | Terminator::RequestChannelNew {
            success, failure, ..
        }
        | Terminator::ReplyNew { success, failure }
        | Terminator::Fork {
            success, failure, ..
        }
        | Terminator::Spawn {
            success, failure, ..
        } => vec![*success, *failure],
        Terminator::RequestSend { sent, closed, .. }
        | Terminator::ChannelSend { sent, closed, .. }
        | Terminator::TypedSend { sent, closed, .. } => vec![*sent, *closed],
        Terminator::RequestReceive { value, end, .. }
        | Terminator::ChannelReceive { value, end, .. }
        | Terminator::TypedReceive { value, end, .. }
        | Terminator::ReplyReceive { value, end, .. } => vec![*value, *end],
        Terminator::ReplyComplete {
            delivered, closed, ..
        } => vec![*delivered, *closed],
    }
}

fn place_transfer(live: &mut HashSet<PlaceId>, operation: &Operation) {
    match operation {
        Operation::PlaceInit(place, _) | Operation::Store(place, _) => {
            live.remove(place);
        }
        Operation::ReadField(place, _)
        | Operation::ArrayGetPlace(place, _)
        | Operation::ArraySetPlace(place, _, _)
        | Operation::Load(place)
        | Operation::Take(place)
        | Operation::CopyPlace(place)
        | Operation::InferredCopyPlace(place)
        | Operation::BorrowBegin(place, _) => {
            live.insert(*place);
        }
        Operation::Call(_, args) => {
            for arg in args {
                if let CallArg::Borrow(place, _) = arg {
                    live.insert(*place);
                }
            }
        }
        _ => {}
    }
}

/// A place that has ever been lent may have an indirect read through a
/// reference. Keep its storage until field-path loan liveness is available.
fn borrowed_places(blocks: &[Block]) -> HashSet<PlaceId> {
    let mut borrowed = HashSet::new();
    for instruction in blocks.iter().flat_map(|block| &block.instructions) {
        match &instruction.operation {
            Operation::BorrowBegin(place, _) => {
                borrowed.insert(*place);
            }
            Operation::Call(_, args) => {
                for arg in args {
                    if let CallArg::Borrow(place, _) = arg {
                        borrowed.insert(*place);
                    }
                }
            }
            _ => {}
        }
    }
    borrowed
}

fn infer_last_closed_place_uses(blocks: &mut [Block]) {
    let borrowed = borrowed_places(blocks);
    let mut incoming = vec![HashSet::new(); blocks.len()];
    loop {
        let mut changed = false;
        for block_id in (0..blocks.len()).rev() {
            let mut live = successors(&blocks[block_id].terminator)
                .into_iter()
                .flat_map(|next| incoming[next.0].iter().copied())
                .collect::<HashSet<_>>();
            for instruction in blocks[block_id].instructions.iter().rev() {
                place_transfer(&mut live, &instruction.operation);
            }
            if incoming[block_id] != live {
                incoming[block_id] = live;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for block_id in 0..blocks.len() {
        let mut live = successors(&blocks[block_id].terminator)
            .into_iter()
            .flat_map(|next| incoming[next.0].iter().copied())
            .collect::<HashSet<_>>();
        for instruction in blocks[block_id].instructions.iter_mut().rev() {
            if let Operation::InferredCopyPlace(place) = instruction.operation
                && !live.contains(&place)
                && !borrowed.contains(&place)
            {
                instruction.operation = Operation::Take(place);
            }
            place_transfer(&mut live, &instruction.operation);
        }
    }
}

fn live_at_terminator(
    blocks: &[Block],
    incoming: &[HashSet<ValueId>],
    end: &Terminator,
) -> HashSet<ValueId> {
    let mut live = HashSet::new();
    let mut edge = |next: BlockId| {
        let mut needed = incoming[next.0].clone();
        for param in &blocks[next.0].params {
            needed.remove(param);
        }
        live.extend(needed);
    };
    match end {
        Terminator::Return(value) => {
            live.insert(*value);
        }
        Terminator::Yield { resume, .. } | Terminator::ScopeJoin { resume, .. } => edge(*resume),
        Terminator::Jump(next, args) => {
            edge(*next);
            live.extend(args.iter().copied());
        }
        Terminator::Branch {
            condition,
            then_block,
            else_block,
        } => {
            edge(*then_block);
            edge(*else_block);
            live.insert(*condition);
        }
        Terminator::Match { subject, arms } => {
            for arm in arms {
                edge(arm.target);
            }
            live.insert(*subject);
        }
        Terminator::ChannelNew {
            capacity,
            success,
            failure,
            ..
        }
        | Terminator::TypedChannelNew {
            capacity,
            success,
            failure,
            ..
        }
        | Terminator::RequestChannelNew {
            capacity,
            success,
            failure,
        } => {
            edge(*success);
            edge(*failure);
            live.insert(*capacity);
        }
        Terminator::ReplyNew { success, failure } => {
            edge(*success);
            edge(*failure);
        }
        Terminator::RequestUnpack { request, next } => {
            edge(*next);
            live.insert(*request);
        }
        Terminator::ProductUnpack { product, next } => {
            edge(*next);
            live.insert(*product);
        }
        Terminator::RequestSend {
            sender,
            request,
            sent,
            closed,
        } => {
            edge(*sent);
            edge(*closed);
            live.extend([*sender, *request]);
        }
        Terminator::RequestReceive {
            receiver,
            value,
            end,
        }
        | Terminator::ChannelReceive {
            receiver,
            value,
            end,
        }
        | Terminator::TypedReceive {
            receiver,
            value,
            end,
        }
        | Terminator::ReplyReceive {
            receiver,
            value,
            end,
        } => {
            edge(*value);
            edge(*end);
            live.insert(*receiver);
        }
        Terminator::Fork {
            source,
            success,
            failure,
        } => {
            edge(*success);
            edge(*failure);
            live.insert(*source);
        }
        Terminator::ChannelSend {
            sender,
            message,
            sent,
            closed,
        }
        | Terminator::TypedSend {
            sender,
            message,
            sent,
            closed,
        } => {
            edge(*sent);
            edge(*closed);
            live.extend([*sender, *message]);
        }
        Terminator::ReplyComplete {
            right,
            value,
            delivered,
            closed,
        } => {
            edge(*delivered);
            edge(*closed);
            live.extend([*right, *value]);
        }
        Terminator::Spawn {
            args,
            success,
            failure,
            ..
        } => {
            edge(*success);
            edge(*failure);
            live.extend(args.iter().copied());
        }
    }
    live
}

pub(super) fn infer_last_closed_uses(blocks: &mut [Block]) {
    infer_last_closed_place_uses(blocks);
    let mut incoming = vec![HashSet::new(); blocks.len()];
    loop {
        let mut changed = false;
        for block_id in (0..blocks.len()).rev() {
            let block = &blocks[block_id];
            let mut live = live_at_terminator(blocks, &incoming, &block.terminator);
            for instruction in block.instructions.iter().rev() {
                if let Some(result) = instruction.result {
                    live.remove(&result);
                }
                live.extend(verification::operands(&instruction.operation));
            }
            for param in &block.params {
                live.remove(param);
            }
            if incoming[block_id] != live {
                incoming[block_id] = live;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for block_id in 0..blocks.len() {
        let mut live = live_at_terminator(blocks, &incoming, &blocks[block_id].terminator);
        for instruction in blocks[block_id].instructions.iter_mut().rev() {
            if let Some(result) = instruction.result {
                live.remove(&result);
            }
            if let Operation::InferredCopy(source) = instruction.operation
                && !live.contains(&source)
            {
                instruction.operation = Operation::Move(source);
            }
            live.extend(verification::operands(&instruction.operation));
        }
    }
}
