//! Stack-effect check for lexical task scopes in checked Core.

use std::collections::{HashSet, VecDeque};

use super::{Function, Operation, Program, TaskScopeId, Terminator};

pub(super) fn verify(program: &Program) -> Result<(), String> {
    for function in &program.functions {
        verify_function(function).map_err(|error| format!("{}: {error}", function.name))?;
    }
    Ok(())
}

fn verify_function(function: &Function) -> Result<(), String> {
    let mut enters = HashSet::new();
    for block in &function.blocks {
        for instruction in &block.instructions {
            if let Operation::ScopeEnter(scope) = instruction.operation
                && !enters.insert(scope)
            {
                return Err(format!("duplicate scope region r{}", scope.0));
            }
        }
    }
    let mut incoming = vec![None::<Vec<TaskScopeId>>; function.blocks.len()];
    incoming[0] = Some(Vec::new());
    let mut ready = VecDeque::from([0usize]);
    while let Some(block_id) = ready.pop_front() {
        let block = &function.blocks[block_id];
        let mut stack = incoming[block_id]
            .clone()
            .expect("queued block has a scope stack");
        for instruction in &block.instructions {
            if let Operation::ScopeEnter(scope) = instruction.operation {
                if stack.contains(&scope) {
                    return Err(format!("scope r{} is already open", scope.0));
                }
                stack.push(scope);
            }
        }
        let targets = match &block.terminator {
            Terminator::ScopeJoin { scope, resume, .. } => {
                if stack.pop() != Some(*scope) || !enters.contains(scope) {
                    return Err(format!(
                        "scope join r{} is not the innermost open scope",
                        scope.0
                    ));
                }
                vec![resume.0]
            }
            Terminator::Return(_) => {
                if !stack.is_empty() {
                    return Err("return leaves a task scope open".into());
                }
                Vec::new()
            }
            Terminator::Yield { resume, .. } => vec![resume.0],
            Terminator::Jump(target, _) => vec![target.0],
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => {
                vec![then_block.0, else_block.0]
            }
            Terminator::Match { arms, .. } => arms.iter().map(|arm| arm.target.0).collect(),
            Terminator::ChannelNew {
                success, failure, ..
            }
            | Terminator::TypedChannelNew {
                success, failure, ..
            }
            | Terminator::RequestChannelNew {
                success, failure, ..
            }
            | Terminator::Fork {
                success, failure, ..
            }
            | Terminator::Spawn {
                success, failure, ..
            }
            | Terminator::ReplyNew { success, failure } => vec![success.0, failure.0],
            Terminator::RequestUnpack { next, .. } | Terminator::ProductUnpack { next, .. } => {
                vec![next.0]
            }
            Terminator::RequestSend { sent, closed, .. }
            | Terminator::TypedSend { sent, closed, .. }
            | Terminator::ChannelSend { sent, closed, .. } => vec![sent.0, closed.0],
            Terminator::RequestReceive { value, end, .. }
            | Terminator::TypedReceive { value, end, .. }
            | Terminator::ChannelReceive { value, end, .. }
            | Terminator::ReplyReceive { value, end, .. } => vec![value.0, end.0],
            Terminator::ReplyComplete {
                delivered, closed, ..
            } => {
                vec![delivered.0, closed.0]
            }
        };
        for target in targets {
            match &incoming[target] {
                None => {
                    incoming[target] = Some(stack.clone());
                    ready.push_back(target);
                }
                Some(existing) if *existing == stack => {}
                Some(_) => {
                    return Err(format!("block{target} merges different open task scopes"));
                }
            }
        }
    }
    Ok(())
}
