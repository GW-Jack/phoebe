use std::collections::{HashSet, VecDeque};

use phoebe_capability::{Action, Edge, Graph, Slot, Target};

use crate::{BlockId, Function, Operation, Terminator, Type, ValueId};

fn value_type(function: &Function, value: ValueId) -> Result<Type, String> {
    function
        .value_types
        .get(value.0)
        .copied()
        .ok_or_else(|| format!("invalid value v{}", value.0))
}

fn locally_defined(local: &HashSet<ValueId>, value: ValueId) -> Result<(), String> {
    if local.contains(&value) {
        Ok(())
    } else {
        Err(format!("v{} is not defined in this block", value.0))
    }
}

fn check_edge(
    function: &Function,
    local: &HashSet<ValueId>,
    target: BlockId,
    args: &[ValueId],
) -> Result<(), String> {
    let block = function
        .blocks
        .get(target.0)
        .ok_or_else(|| format!("invalid block b{}", target.0))?;
    if args.len() != block.params.len() {
        return Err(format!("b{} argument count mismatch", target.0));
    }
    for (&arg, &param) in args.iter().zip(&block.params) {
        locally_defined(local, arg)?;
        if value_type(function, arg)? != value_type(function, param)? {
            return Err(format!("b{} argument type mismatch", target.0));
        }
    }
    Ok(())
}

fn reachable(function: &Function) -> Result<(), String> {
    let mut seen = vec![false; function.blocks.len()];
    let mut queue = VecDeque::from([0]);
    seen[0] = true;
    while let Some(block) = queue.pop_front() {
        let targets = match &function.blocks[block].terminator {
            Terminator::Return(_) => [None, None],
            Terminator::Jump { target, .. } => [Some(*target), None],
            Terminator::Branch {
                when_true,
                when_false,
                ..
            } => [Some(*when_true), Some(*when_false)],
        };
        for target in targets.into_iter().flatten() {
            if !seen[target.0] {
                seen[target.0] = true;
                queue.push_back(target.0);
            }
        }
    }
    if let Some(index) = seen.iter().position(|seen| !seen) {
        return Err(format!("unreachable block b{index}"));
    }
    Ok(())
}

fn move_edge(
    function: &Function,
    slots: &[Option<Slot>],
    target: BlockId,
    args: &[ValueId],
) -> Edge {
    let moves = args
        .iter()
        .zip(&function.blocks[target.0].params)
        .filter_map(|(arg, param)| slots[arg.0].zip(slots[param.0]))
        .collect::<Vec<_>>();
    Edge {
        target: Target::Block(target.0),
        actions: if moves.is_empty() {
            Vec::new()
        } else {
            vec![Action::MoveGroup(moves)]
        },
    }
}

/// Verify a parametric definition without choosing concrete types or ABI
/// shapes. Opaque parameters are checked as exact affine owners.
pub fn verify(function: &Function) -> Result<(), String> {
    if function.blocks.is_empty() {
        return Err("generic function has no entry block".into());
    }
    for ty in function
        .params
        .iter()
        .chain(std::iter::once(&function.result))
        .chain(&function.value_types)
    {
        if let Type::Param(index) = ty
            && *index >= function.type_params
        {
            return Err(format!("unbound type parameter {index}"));
        }
    }
    if function.blocks[0].params.len() != function.params.len() {
        return Err("generic entry parameter count mismatch".into());
    }
    for (&value, &expected) in function.blocks[0].params.iter().zip(&function.params) {
        if value_type(function, value)? != expected {
            return Err("generic entry parameter type mismatch".into());
        }
    }

    let mut defined = vec![false; function.value_types.len()];
    for (block_id, block) in function.blocks.iter().enumerate() {
        let mut local = HashSet::new();
        for &param in &block.params {
            let Some(entry) = defined.get_mut(param.0) else {
                return Err(format!("b{block_id}: invalid parameter v{}", param.0));
            };
            if *entry {
                return Err(format!("v{} has multiple definitions", param.0));
            }
            *entry = true;
            local.insert(param);
        }
        for instruction in &block.instructions {
            let input = match instruction.operation {
                Operation::Move(value) | Operation::CopyScalar(value) => value,
            };
            locally_defined(&local, input)?;
            let input_type = value_type(function, input)?;
            if matches!(instruction.operation, Operation::CopyScalar(_))
                && matches!(input_type, Type::Param(_))
            {
                return Err("opaque type parameter cannot be copied".into());
            }
            if value_type(function, instruction.result)? != input_type {
                return Err(format!("v{} result type mismatch", instruction.result.0));
            }
            let entry = &mut defined[instruction.result.0];
            if *entry {
                return Err(format!(
                    "v{} has multiple definitions",
                    instruction.result.0
                ));
            }
            *entry = true;
            local.insert(instruction.result);
        }
        match &block.terminator {
            Terminator::Return(value) => {
                locally_defined(&local, *value)?;
                if value_type(function, *value)? != function.result {
                    return Err(format!("b{block_id}: return type mismatch"));
                }
            }
            Terminator::Jump { target, args } => check_edge(function, &local, *target, args)?,
            Terminator::Branch {
                condition,
                when_true,
                true_args,
                when_false,
                false_args,
            } => {
                locally_defined(&local, *condition)?;
                if value_type(function, *condition)? != Type::Bool {
                    return Err(format!("b{block_id}: branch condition must be Bool"));
                }
                check_edge(function, &local, *when_true, true_args)?;
                check_edge(function, &local, *when_false, false_args)?;
            }
        }
    }
    if let Some(index) = defined.iter().position(|defined| !defined) {
        return Err(format!("v{index} has no definition"));
    }
    reachable(function)?;

    let mut slots = Vec::with_capacity(function.value_types.len());
    let mut slot_count = 0;
    for ty in &function.value_types {
        if matches!(ty, Type::Param(_)) {
            slots.push(Some(Slot(slot_count)));
            slot_count += 1;
        } else {
            slots.push(None);
        }
    }
    let entry_live = function.blocks[0]
        .params
        .iter()
        .filter_map(|value| slots[value.0])
        .collect();
    let blocks = function
        .blocks
        .iter()
        .map(|block| {
            let actions = block
                .instructions
                .iter()
                .filter_map(|instruction| match instruction.operation {
                    Operation::Move(value) => slots[value.0]
                        .zip(slots[instruction.result.0])
                        .map(|(from, to)| Action::Move { from, to }),
                    Operation::CopyScalar(_) => None,
                })
                .collect();
            let exits = match &block.terminator {
                Terminator::Return(value) => vec![Edge {
                    target: Target::Return,
                    actions: slots[value.0]
                        .map(|slot| vec![Action::Transfer(slot)])
                        .unwrap_or_default(),
                }],
                Terminator::Jump { target, args } => {
                    vec![move_edge(function, &slots, *target, args)]
                }
                Terminator::Branch {
                    when_true,
                    true_args,
                    when_false,
                    false_args,
                    ..
                } => vec![
                    move_edge(function, &slots, *when_true, true_args),
                    move_edge(function, &slots, *when_false, false_args),
                ],
            };
            phoebe_capability::Block { actions, exits }
        })
        .collect();
    phoebe_capability::verify(&Graph {
        slots: slot_count,
        entry_live,
        blocks,
    })
    .map_err(|error| format!("{}: {error}", function.name))?;
    Ok(())
}
