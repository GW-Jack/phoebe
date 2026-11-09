//! Static source-attributed cost sites in checked Core. This is not a dynamic
//! execution count or a claim about final machine instructions.

use super::*;

fn storage_type_name(ty: Type) -> String {
    match ty {
        Type::ArrayI64(len) => format!("[I64; {len}]"),
        _ => format!("{ty:?}"),
    }
}

impl Program {
    pub fn cost_events(&self) -> Vec<CostEvent> {
        let capability_layout = verification::capability_layout::Layout::new(&self.data);
        let storage = StoragePlan::for_program(self);
        let mut events = Vec::new();
        for (function_id, function) in self.functions.iter().enumerate() {
            for (block_id, block) in function.blocks.iter().enumerate() {
                for (instruction_id, instruction) in block.instructions.iter().enumerate() {
                    if let Operation::ScopeEnter(_) = instruction.operation {
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: instruction_id,
                            span: instruction.span,
                            kind: "scope.enter",
                            type_name: "TaskScope".into(),
                            reason: "lexical scope",
                        });
                    }
                    if let Operation::DestBeginVariant(place, _) = instruction.operation {
                        let Type::Data(id) = function.place_types[place.0] else {
                            unreachable!("checked variant destination")
                        };
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: instruction_id,
                            span: instruction.span,
                            kind: "destination.zero",
                            type_name: self.data[id.0].name.clone(),
                            reason: "initialize active variant destination",
                        });
                    }
                    if let Operation::DestField(place, index, _) = instruction.operation {
                        let Type::Data(id) = function.place_types[place.0] else {
                            unreachable!("checked destination field")
                        };
                        let definition = &self.data[id.0];
                        let ty = if definition.fields.is_empty() {
                            definition.payloads[index].expect("checked variant payload")
                        } else {
                            definition.fields[index].1
                        };
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: instruction_id,
                            span: instruction.span,
                            kind: "destination.write",
                            type_name: storage_type_name(ty),
                            reason: "write unpublished field or payload",
                        });
                    }
                    let copy_ty = match instruction.operation {
                        Operation::Copy(value) | Operation::InferredCopy(value) => {
                            Some(function.value_types[value.0])
                        }
                        Operation::CopyPlace(place) | Operation::InferredCopyPlace(place) => {
                            Some(function.place_types[place.0])
                        }
                        _ => None,
                    };
                    if let Some(ty @ (Type::Data(_) | Type::ArrayI64(_))) = copy_ty {
                        let type_name = match ty {
                            Type::Data(id) => self.data[id.0].name.clone(),
                            Type::ArrayI64(len) => format!("[I64; {len}]"),
                            _ => unreachable!(),
                        };
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: instruction_id,
                            span: instruction.span,
                            kind: if instruction
                                .result
                                .is_some_and(|result| storage.borrows_copy(function_id, result))
                            {
                                "storage.borrow"
                            } else if storage.shares_type(ty)
                                && matches!(
                                    instruction.operation,
                                    Operation::Copy(_) | Operation::CopyPlace(_)
                                )
                            {
                                "storage.copy_words"
                            } else if storage.shares_type(ty) {
                                "storage.retain"
                            } else {
                                "copy.value"
                            },
                            type_name,
                            reason: if matches!(
                                instruction.operation,
                                Operation::InferredCopy(_) | Operation::InferredCopyPlace(_)
                            ) {
                                "inferred value use"
                            } else {
                                "explicit"
                            },
                        });
                        if storage.shares_type(ty)
                            && matches!(
                                instruction.operation,
                                Operation::Copy(_) | Operation::CopyPlace(_)
                            )
                        {
                            events.push(CostEvent {
                                function: function.name.clone(),
                                block: block_id,
                                instruction: instruction_id,
                                span: instruction.span,
                                kind: "storage.allocate",
                                type_name: storage_type_name(ty),
                                reason: "explicit independent copy",
                            });
                        }
                    }
                    if let Operation::ArrayI64(ref values) = instruction.operation
                        && storage.shares_array(values.len())
                    {
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: instruction_id,
                            span: instruction.span,
                            kind: "storage.allocate",
                            type_name: format!("[I64; {}]", values.len()),
                            reason: "shared array construction",
                        });
                    }
                    if let Operation::ArraySetPlace(place, _, _) = instruction.operation
                        && storage.shares_type(function.place_types[place.0])
                    {
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: instruction_id,
                            span: instruction.span,
                            kind: "storage.cow",
                            type_name: storage_type_name(function.place_types[place.0]),
                            reason: "write may allocate and copy shared words",
                        });
                    }
                    let replaced_owner = match instruction.operation {
                        Operation::PlaceInit(place, _) | Operation::Store(place, _)
                            if storage.shares_type(function.place_types[place.0]) =>
                        {
                            Some(function.place_types[place.0])
                        }
                        Operation::CloseCaps(value)
                            if storage.shares_type(function.value_types[value.0]) =>
                        {
                            Some(function.value_types[value.0])
                        }
                        _ => instruction
                            .result
                            .filter(|result| {
                                storage.shares_type(function.value_types[result.0])
                                    && !storage.borrows_copy(function_id, *result)
                                    && !function.blocks[0].params.iter().enumerate().any(
                                        |(param, id)| {
                                            *id == *result
                                                && storage.borrows_param(function_id, param)
                                        },
                                    )
                            })
                            .map(|result| function.value_types[result.0]),
                    };
                    if let Some(ty) = replaced_owner {
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: instruction_id,
                            span: instruction.span,
                            kind: "storage.release",
                            type_name: storage_type_name(ty),
                            reason: "owner slot replacement (may be empty)",
                        });
                    }
                    let projected_ty = match instruction.operation {
                        Operation::Field(owner, index) => match function.value_types[owner.0] {
                            Type::Data(id) => self.data[id.0].fields.get(index).map(|(_, ty)| *ty),
                            _ => None,
                        },
                        Operation::ReadField(place, index) => match function.place_types[place.0] {
                            Type::Data(id) => self.data[id.0].fields.get(index).map(|(_, ty)| *ty),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(ty @ (Type::Data(_) | Type::ArrayI64(_))) = projected_ty {
                        let type_name = match ty {
                            Type::Data(id) => self.data[id.0].name.clone(),
                            Type::ArrayI64(len) => format!("[I64; {len}]"),
                            _ => unreachable!(),
                        };
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: instruction_id,
                            span: instruction.span,
                            kind: "copy.value",
                            type_name,
                            reason: "inferred field value",
                        });
                    }
                    let close = match instruction.operation {
                        Operation::CloseHandle(value)
                        | Operation::CloseReply(value)
                        | Operation::CloseRequest(value) => Some((value, "owned capability close")),
                        Operation::CloseCaps(value) => Some((value, "structural capability close")),
                        _ => None,
                    };
                    if let Some((value, reason)) = close {
                        let ty = function.value_types[value.0];
                        if !matches!(instruction.operation, Operation::CloseCaps(_))
                            || capability_layout.leaves(ty) > 0
                        {
                            let type_name = match ty {
                                Type::Data(id) => self.data[id.0].name.clone(),
                                _ => format!("{ty:?}"),
                            };
                            events.push(CostEvent {
                                function: function.name.clone(),
                                block: block_id,
                                instruction: instruction_id,
                                span: instruction.span,
                                kind: "channel.op",
                                type_name,
                                reason,
                            });
                        }
                    }
                }
                let exit_span = block
                    .instructions
                    .last()
                    .map(|instruction| instruction.span)
                    .unwrap_or(Span { start: 0, end: 0 });
                if let Terminator::Return(_) = block.terminator {
                    for (value_id, ty) in function.value_types.iter().enumerate() {
                        if !storage.shares_type(*ty)
                            || storage.borrows_copy(function_id, ValueId(value_id))
                            || function.blocks[0]
                                .params
                                .iter()
                                .enumerate()
                                .any(|(param, id)| {
                                    id.0 == value_id && storage.borrows_param(function_id, param)
                                })
                        {
                            continue;
                        }
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: block.instructions.len(),
                            span: exit_span,
                            kind: "storage.release",
                            type_name: storage_type_name(*ty),
                            reason: "frame exit (only live owner slots)",
                        });
                    }
                    for ty in function
                        .place_types
                        .iter()
                        .filter(|ty| storage.shares_type(**ty))
                    {
                        events.push(CostEvent {
                            function: function.name.clone(),
                            block: block_id,
                            instruction: block.instructions.len(),
                            span: exit_span,
                            kind: "storage.release",
                            type_name: storage_type_name(*ty),
                            reason: "frame exit (only live owner slots)",
                        });
                    }
                }
                if let Terminator::Jump(target, ref args) = block.terminator {
                    for (_arg, param) in args.iter().zip(&function.blocks[target.0].params) {
                        let ty = function.value_types[param.0];
                        if storage.shares_type(ty) {
                            events.push(CostEvent {
                                function: function.name.clone(),
                                block: block_id,
                                instruction: block.instructions.len(),
                                span: exit_span,
                                kind: "storage.release",
                                type_name: storage_type_name(ty),
                                reason: "block parameter replacement (may be empty)",
                            });
                        }
                    }
                }
                if let Terminator::Yield {
                    span, automatic, ..
                } = block.terminator
                {
                    events.push(CostEvent {
                        function: function.name.clone(),
                        block: block_id,
                        instruction: block.instructions.len(),
                        span,
                        kind: "task.yield",
                        type_name: "Task".into(),
                        reason: if automatic {
                            "loop backedge"
                        } else {
                            "explicit"
                        },
                    });
                }
                if let Terminator::Spawn { span, .. } = block.terminator {
                    events.push(CostEvent {
                        function: function.name.clone(),
                        block: block_id,
                        instruction: block.instructions.len(),
                        span,
                        kind: "task.create",
                        type_name: "Task".into(),
                        reason: "explicit spawn",
                    });
                }
                if let Terminator::ScopeJoin { span, .. } = block.terminator {
                    events.push(CostEvent {
                        function: function.name.clone(),
                        block: block_id,
                        instruction: block.instructions.len(),
                        span,
                        kind: "scope.join",
                        type_name: "TaskScope".into(),
                        reason: "lexical scope",
                    });
                }
                let channel_creation = match block.terminator {
                    Terminator::ChannelNew { span, .. } => Some((span, "I64".to_owned())),
                    Terminator::TypedChannelNew { message, span, .. } => {
                        let ty = message.value_type();
                        let type_name = match ty {
                            Type::Data(id) => self.data[id.0].name.clone(),
                            _ => format!("{ty:?}"),
                        };
                        Some((span, type_name))
                    }
                    _ => None,
                };
                if let Some((span, type_name)) = channel_creation {
                    events.push(CostEvent {
                        function: function.name.clone(),
                        block: block_id,
                        instruction: block.instructions.len(),
                        span,
                        kind: "channel.op",
                        type_name,
                        reason: "channel construction",
                    });
                }
            }
        }
        events
    }
}
