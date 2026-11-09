//! A conservative, program-wide physical storage choice for closed arrays.
//! Both the interpreter and native ABI consult this after Core verification.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoragePlan {
    managed_arrays: [bool; 257],
    borrowed_params: Vec<Vec<bool>>,
    borrowed_copies: Vec<Vec<bool>>,
}

impl StoragePlan {
    pub fn for_program(program: &Program) -> Self {
        let mut managed_arrays = [false; 257];
        // The first native implementation owns bare array pointers in direct
        // frames. Matches may select an early return, including `?`; every
        // return releases the owners still held by that frame. Data payloads
        // and retained task frames keep their inline ABI.
        let simple_control = program.functions.iter().all(|function| {
            function.blocks.iter().all(|block| {
                matches!(
                    block.terminator,
                    Terminator::Return(_)
                        | Terminator::Jump(_, _)
                        | Terminator::Branch { .. }
                        | Terminator::Match { .. }
                ) && block.instructions.iter().all(|instruction| {
                    !matches!(
                        instruction.operation,
                        Operation::ScopeEnter(_) | Operation::GenericCall { .. }
                    )
                })
            })
        });
        let bare_arrays = program.data.iter().all(|definition| {
            definition
                .fields
                .iter()
                .map(|(_, ty)| ty)
                .chain(definition.payloads.iter().flatten())
                .all(|ty| !matches!(ty, Type::ArrayI64(_)))
        });
        if simple_control && bare_arrays {
            for function in &program.functions {
                for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
                    let copied = match instruction.operation {
                        Operation::InferredCopy(value) => Some(function.value_types[value.0]),
                        Operation::InferredCopyPlace(place) | Operation::Load(place) => {
                            Some(function.place_types[place.0])
                        }
                        _ => None,
                    };
                    if let Some(Type::ArrayI64(len @ 256)) = copied {
                        managed_arrays[len] = true;
                    }
                }
            }
        }
        let mut plan = Self {
            managed_arrays,
            borrowed_params: program
                .functions
                .iter()
                .map(|function| vec![false; function.blocks[0].params.len()])
                .collect(),
            borrowed_copies: program
                .functions
                .iter()
                .map(|function| vec![false; function.value_types.len()])
                .collect(),
        };
        if !plan.has_shared_arrays() {
            return plan;
        }
        for (function_id, function) in program.functions.iter().enumerate() {
            let managed_params = function.blocks[0]
                .params
                .iter()
                .filter(|param| plan.shares_type(function.value_types[param.0]))
                .count();
            if managed_params != 1 {
                continue;
            }
            for (index, param) in function.blocks[0].params.iter().enumerate() {
                if !plan.shares_type(function.value_types[param.0]) {
                    continue;
                }
                let read_only = function.blocks.iter().all(|block| {
                    block.instructions.iter().all(|instruction| {
                        let uses = verification::operands(&instruction.operation);
                        !uses.contains(param)
                            || matches!(
                                instruction.operation,
                                Operation::ArrayGet(value, _) | Operation::ArrayLen(value)
                                    if value == *param
                            )
                    }) && match &block.terminator {
                        Terminator::Return(value) => *value != *param,
                        Terminator::Jump(_, args) => !args.contains(param),
                        Terminator::Branch { condition, .. } => *condition != *param,
                        _ => false,
                    }
                });
                plan.borrowed_params[function_id][index] = read_only;
            }
        }
        for (function_id, function) in program.functions.iter().enumerate() {
            for block in &function.blocks {
                for pair in block.instructions.windows(2) {
                    let (copy, call) = (&pair[0], &pair[1]);
                    let Some(result) = copy.result else { continue };
                    if !plan.shares_type(function.value_types[result.0])
                        || !matches!(
                            copy.operation,
                            Operation::InferredCopy(_) | Operation::InferredCopyPlace(_)
                        )
                    {
                        continue;
                    }
                    let Operation::Call(target, args) = &call.operation else {
                        continue;
                    };
                    let uses = args
                        .iter()
                        .enumerate()
                        .filter_map(|(index, arg)| {
                            matches!(arg, CallArg::Value(value) if *value == result)
                                .then_some(index)
                        })
                        .collect::<Vec<_>>();
                    if uses.len() == 1 && plan.borrows_param(target.0, uses[0]) {
                        plan.borrowed_copies[function_id][result.0] = true;
                    }
                }
            }
        }
        plan
    }

    pub fn shares_array(&self, len: usize) -> bool {
        self.managed_arrays.get(len).copied().unwrap_or(false)
    }

    pub fn shares_type(&self, ty: Type) -> bool {
        matches!(ty, Type::ArrayI64(len) if self.shares_array(len))
    }

    pub fn has_shared_arrays(&self) -> bool {
        self.managed_arrays.iter().any(|shared| *shared)
    }

    pub fn borrows_param(&self, function: usize, index: usize) -> bool {
        self.borrowed_params
            .get(function)
            .and_then(|params| params.get(index))
            .copied()
            .unwrap_or(false)
    }

    pub fn borrows_copy(&self, function: usize, result: ValueId) -> bool {
        self.borrowed_copies
            .get(function)
            .and_then(|values| values.get(result.0))
            .copied()
            .unwrap_or(false)
    }
}

impl CheckedProgram {
    pub fn storage_plan(&self) -> StoragePlan {
        StoragePlan::for_program(self.as_program())
    }
}

impl CheckedLinkedProgram {
    pub fn storage_plan(&self) -> StoragePlan {
        StoragePlan::for_program(self.program())
    }
}

impl VerifiedProgram<'_> {
    pub fn storage_plan(self) -> StoragePlan {
        StoragePlan::for_program(self.as_program())
    }
}
