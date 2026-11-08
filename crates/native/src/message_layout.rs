//! Translate checked, backend-independent message layouts to the current
//! runtime's typed queue descriptor. No descriptor is built at send time.

use phoebe_abi::{ClosePlan, MessageLayout};
use phoebe_core::{DataId, Type};
use phoebe_runtime::TypedRightPlan;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeMessageDescriptor {
    pub type_id: u64,
    pub width: usize,
    pub rights: TypedRightPlan,
}

/// Type IDs are unique within one checked linked Program. The high bit is
/// reserved for its dense nominal DataId space; low values name built-ins.
/// A descriptor must never be reused across independently linked programs.
fn type_id(ty: Type) -> Result<u64, String> {
    match ty {
        Type::I64 => Ok(1),
        Type::Bool => Ok(2),
        Type::Unit => Ok(3),
        Type::RequestI64 => Ok(4),
        Type::ReplyToI64 => Ok(5),
        Type::ResourceError => Ok(6),
        Type::Data(DataId(id)) => {
            let id = u64::try_from(id).map_err(|_| "message data ID exceeds runtime type ID")?;
            if id >= (1u64 << 63) {
                return Err("message data ID exceeds runtime type ID".into());
            }
            Ok((1u64 << 63) | id)
        }
        _ => Err("message type is not supported by the typed runtime".into()),
    }
}

fn rights(plan: &ClosePlan, depth: usize) -> Result<TypedRightPlan, String> {
    if depth >= 64 {
        return Err("message ownership plan exceeds runtime depth limit".into());
    }
    match plan {
        ClosePlan::Empty => Ok(TypedRightPlan::Empty),
        ClosePlan::Leaf {
            part,
            kind: Type::ReplyToI64,
        } => Ok(TypedRightPlan::ReplyTo { part: *part }),
        ClosePlan::Leaf { .. } => {
            Err("message contains a right unsupported by the typed runtime".into())
        }
        ClosePlan::Sequence(plans) => Ok(TypedRightPlan::Sequence(
            plans
                .iter()
                .map(|plan| rights(plan, depth + 1))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        ClosePlan::Tagged { tag_part, arms } => Ok(TypedRightPlan::Tagged {
            tag_part: *tag_part,
            arms: arms
                .iter()
                .map(|arm| rights(arm, depth + 1))
                .collect::<Result<Vec<_>, _>>()?,
        }),
    }
}

pub fn runtime_message_descriptor(
    layout: &MessageLayout,
) -> Result<RuntimeMessageDescriptor, String> {
    Ok(RuntimeMessageDescriptor {
        type_id: type_id(layout.ty())?,
        width: layout.shape().parts().len(),
        rights: rights(layout.close_plan(), 0)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoebe_abi::Shapes;
    use phoebe_core::{DataDef, Program};
    use phoebe_runtime::{Context, Scheduler, Task, TypedReceiveResult, TypedSendResult};
    use std::task::Poll;

    #[test]
    fn same_width_types_keep_distinct_identity_and_right_plan() {
        let program = Program {
            data: vec![
                DataDef {
                    name: "Number".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![("value".into(), Type::I64)],
                },
                DataDef {
                    name: "Reply".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![("right".into(), Type::ReplyToI64)],
                },
            ],
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        let number =
            runtime_message_descriptor(&shapes.message_layout(Type::Data(DataId(0))).unwrap())
                .unwrap();
        let reply =
            runtime_message_descriptor(&shapes.message_layout(Type::Data(DataId(1))).unwrap())
                .unwrap();
        assert_eq!(number.width, reply.width);
        assert_ne!(number.type_id, reply.type_id);
        assert_eq!(number.rights, TypedRightPlan::Empty);
        assert_eq!(reply.rights, TypedRightPlan::ReplyTo { part: 0 });
    }

    #[test]
    fn nested_sum_preserves_active_arm_offsets_and_reverse_close_order() {
        let program = Program {
            data: vec![
                DataDef {
                    name: "MaybeReply".into(),
                    variants: vec!["None".into(), "Some".into()],
                    payloads: vec![None, Some(Type::ReplyToI64)],
                    fields: vec![],
                },
                DataDef {
                    name: "Packet".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("maybe".into(), Type::Data(DataId(0))),
                        ("tail".into(), Type::RequestI64),
                    ],
                },
            ],
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        let descriptor =
            runtime_message_descriptor(&shapes.message_layout(Type::Data(DataId(1))).unwrap())
                .unwrap();
        assert_eq!(descriptor.width, 4);
        assert_eq!(
            descriptor.rights,
            TypedRightPlan::Sequence(vec![
                TypedRightPlan::ReplyTo { part: 3 },
                TypedRightPlan::Tagged {
                    tag_part: 0,
                    arms: vec![TypedRightPlan::Empty, TypedRightPlan::ReplyTo { part: 1 }],
                },
            ])
        );
    }

    #[test]
    fn unsupported_rights_and_references_are_rejected() {
        let program = Program {
            data: vec![DataDef {
                name: "SenderPacket".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![("sender".into(), Type::SenderI64)],
            }],
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        let sender = shapes.message_layout(Type::Data(DataId(0))).unwrap();
        assert!(runtime_message_descriptor(&sender).is_err());
        assert!(shapes.message_layout(Type::RefI64).is_err());
    }

    struct DescriptorTask {
        descriptor: Option<RuntimeMessageDescriptor>,
    }

    impl Task for DescriptorTask {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            let descriptor = self.descriptor.take().unwrap();
            let (sender, receiver) = context
                .typed_right_channel(1, descriptor.type_id, descriptor.width, descriptor.rights)
                .map_err(|error| error.to_string())?;
            let (right, reply_receiver) = context.reply()?;
            let message = [1, right.token().native_word()? as u64];
            assert_eq!(
                context.send_typed_words(&sender, descriptor.type_id, &message)?,
                TypedSendResult::Sent
            );
            let mut output = [0; 2];
            assert_eq!(
                context.receive_typed_words(&receiver, descriptor.type_id, &mut output)?,
                TypedReceiveResult::Value
            );
            assert_eq!(output, message);
            context.close_reply_to(right)?;
            context.cancel_reply(reply_receiver)?;
            context.close_sender(sender)?;
            context.close_receiver(receiver)?;
            Ok(Poll::Ready(7))
        }
    }

    #[test]
    fn checked_abi_descriptor_drives_runtime_tagged_transfer() {
        let program = Program {
            data: vec![DataDef {
                name: "MaybeReply".into(),
                variants: vec!["None".into(), "Some".into()],
                payloads: vec![None, Some(Type::ReplyToI64)],
                fields: vec![],
            }],
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        let descriptor =
            runtime_message_descriptor(&shapes.message_layout(Type::Data(DataId(0))).unwrap())
                .unwrap();
        let mut scheduler = Scheduler::new(
            DescriptorTask {
                descriptor: Some(descriptor),
            },
            1,
            1,
            1,
        )
        .unwrap();
        assert_eq!(scheduler.run().unwrap(), 7);
        scheduler.verify().unwrap();
        assert_eq!(scheduler.open_handles(), 0);
    }
}
