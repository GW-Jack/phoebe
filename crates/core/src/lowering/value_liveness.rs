//! Backward SSA liveness for choosing a transfer on a closed value's last use.

use super::*;
use std::collections::HashSet;

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
