pub use phoebe_message::{RuntimeMessageDescriptor, runtime_message_descriptor};

#[cfg(test)]
mod tests {
    use super::*;
    use phoebe_abi::Shapes;
    use phoebe_core::{DataDef, DataId, Program, Type};
    use phoebe_runtime::{
        Context, Scheduler, Task, TypedReceiveResult, TypedRightPlan, TypedSendResult,
    };
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
