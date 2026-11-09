//! Static source-attributed cost sites in checked Core. This is not a dynamic
//! execution count or a claim about final machine instructions.

use super::*;

impl Program {
    pub fn cost_events(&self) -> Vec<CostEvent> {
        let capability_layout = verification::capability_layout::Layout::new(&self.data);
        let mut events = Vec::new();
        for function in &self.functions {
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
                            kind: "copy.value",
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
