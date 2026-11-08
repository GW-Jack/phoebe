//! Backend independent ABI plans for concrete and generic Phoebe code.

pub use phoebe_layout::{CapabilityPart, ClosePlan, MessageLayout, Part, Shape, Shapes};

pub mod generic;

#[cfg(test)]
mod tests {
    use super::*;
    use phoebe_core::{self as core, DataId, MessageType, Type};

    #[test]
    fn distinct_nominal_products_share_transport_shape() {
        let source = "data First { x: I64, y: Bool }\ndata Second { a: I64, b: Bool }\nfn main() -> I64 = 0\n";
        let program = core::lower(&phoebe_syntax::parse(source).unwrap()).unwrap();
        let shapes = Shapes::new(&program).unwrap();
        assert_eq!(
            shapes.shape(Type::Data(DataId(0))).unwrap(),
            shapes.shape(Type::Data(DataId(1))).unwrap()
        );
        assert_eq!(
            shapes.shape(Type::Data(DataId(0))).unwrap().parts(),
            &[Part::Word, Part::Byte]
        );
        assert_ne!(
            shapes.shape(Type::I64).unwrap(),
            shapes.shape(Type::Bool).unwrap()
        );
    }

    #[test]
    fn unit_has_no_transport_parts() {
        let program = core::lower(&phoebe_syntax::parse("fn main() -> I64 = 0").unwrap()).unwrap();
        let shapes = Shapes::new(&program).unwrap();
        assert!(shapes.parts(Type::Unit).unwrap().is_empty());
    }

    #[test]
    fn sum_payload_uses_word_transport_even_for_boolean() {
        let source = "data Maybe { None, Some(value: Bool) }\nfn main() -> I64 = 0\n";
        let program = core::lower(&phoebe_syntax::parse(source).unwrap()).unwrap();
        let shapes = Shapes::new(&program).unwrap();
        assert_eq!(
            shapes.shape(Type::Data(DataId(0))).unwrap().parts(),
            &[Part::Word, Part::Word]
        );
    }

    #[test]
    fn recursive_layout_is_rejected_before_shape_construction() {
        let program = core::Program {
            data: vec![core::DataDef {
                name: "Loop".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![("next".into(), Type::Data(DataId(0)))],
            }],
            functions: vec![],
        };
        assert!(Shapes::new(&program).is_err());
    }

    #[test]
    fn nested_capability_offsets_follow_flattened_product_parts() {
        let program = core::Program {
            data: vec![
                core::DataDef {
                    name: "Pair".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("sender".into(), Type::SenderI64),
                        ("request".into(), Type::RequestI64),
                    ],
                },
                core::DataDef {
                    name: "Envelope".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("number".into(), Type::I64),
                        ("pair".into(), Type::Data(DataId(0))),
                        ("receiver".into(), Type::ReceiverI64),
                    ],
                },
            ],
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        assert_eq!(shapes.parts(Type::Data(DataId(1))).unwrap().len(), 5);
        assert_eq!(
            shapes.capability_parts(Type::Data(DataId(1))).unwrap().as_ref(),
            &[
                CapabilityPart {
                    part: 1,
                    kind: Type::SenderI64
                },
                CapabilityPart {
                    part: 3,
                    kind: Type::ReplyToI64
                },
                CapabilityPart {
                    part: 4,
                    kind: Type::ReceiverI64
                },
            ]
        );
    }

    #[test]
    fn typed_endpoint_rights_survive_nested_product_layout() {
        let sender = Type::TypedSender(MessageType::Bool);
        let receiver = Type::TypedReceiver(MessageType::Data(DataId(0)));
        let program = core::Program {
            data: vec![
                core::DataDef {
                    name: "Payload".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![("value".into(), Type::I64)],
                },
                core::DataDef {
                    name: "Bundle".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("number".into(), Type::I64),
                        ("sender".into(), sender),
                        ("receiver".into(), receiver),
                    ],
                },
            ],
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        assert_eq!(
            shapes
                .capability_parts(Type::Data(DataId(1)))
                .unwrap()
                .as_ref(),
            &[
                CapabilityPart {
                    part: 1,
                    kind: sender
                },
                CapabilityPart {
                    part: 2,
                    kind: receiver
                },
            ]
        );
        assert_eq!(
            shapes.capability_parts(sender).unwrap().as_ref(),
            &[CapabilityPart {
                part: 0,
                kind: sender
            }]
        );
    }

    #[test]
    fn tagged_close_plan_preserves_product_reverse_order_and_variant_offset() {
        let program = core::Program {
            data: vec![
                core::DataDef {
                    name: "MaybeSender".into(),
                    variants: vec!["None".into(), "Some".into()],
                    payloads: vec![None, Some(Type::SenderI64)],
                    fields: vec![],
                },
                core::DataDef {
                    name: "Envelope".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("n".into(), Type::I64),
                        ("maybe".into(), Type::Data(DataId(0))),
                        ("reply".into(), Type::ReplyToI64),
                    ],
                },
            ],
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        assert_eq!(shapes.parts(Type::Data(DataId(1))).unwrap().len(), 4);
        assert_eq!(
            shapes.close_plan(Type::Data(DataId(1))).unwrap(),
            ClosePlan::Sequence(vec![
                ClosePlan::Leaf {
                    part: 3,
                    kind: Type::ReplyToI64,
                },
                ClosePlan::Tagged {
                    tag_part: 1,
                    arms: vec![
                        ClosePlan::Empty,
                        ClosePlan::Leaf {
                            part: 2,
                            kind: Type::SenderI64,
                        },
                    ],
                },
            ])
        );
    }

    #[test]
    fn message_layout_distinguishes_equal_transport_and_rejects_nested_reference() {
        let program = core::Program {
            data: vec![
                core::DataDef {
                    name: "Closed".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![("value".into(), Type::I64)],
                },
                core::DataDef {
                    name: "Right".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![("sender".into(), Type::SenderI64)],
                },
                core::DataDef {
                    name: "Borrowed".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![("reference".into(), Type::RefI64)],
                },
                core::DataDef {
                    name: "NestedBorrow".into(),
                    variants: vec!["None".into(), "Some".into()],
                    payloads: vec![None, Some(Type::Data(DataId(2)))],
                    fields: vec![],
                },
            ],
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        let closed = shapes.message_layout(Type::Data(DataId(0))).unwrap();
        let right = shapes.message_layout(Type::Data(DataId(1))).unwrap();
        assert_eq!(closed.shape(), right.shape());
        assert_ne!(closed.ty(), right.ty());
        assert_eq!(closed.close_plan(), &ClosePlan::Empty);
        assert_eq!(
            right.close_plan().clone(),
            ClosePlan::Leaf {
                part: 0,
                kind: Type::SenderI64,
            }
        );
        assert!(shapes.message_layout(Type::RefI64).is_err());
        assert!(shapes.message_layout(Type::ReplyReceiverI64).is_err());
        assert!(shapes.message_layout(Type::Data(DataId(3))).is_err());
    }

    #[test]
    fn message_layout_keeps_tagged_cleanup_and_zero_width_occupancy() {
        let program = core::Program {
            data: vec![core::DataDef {
                name: "MaybeRight".into(),
                variants: vec!["None".into(), "Some".into()],
                payloads: vec![None, Some(Type::ReplyToI64)],
                fields: vec![],
            }],
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        assert_eq!(
            shapes.message_layout(Type::Unit).unwrap().shape().parts(),
            &[]
        );
        assert_eq!(
            shapes
                .message_layout(Type::Data(DataId(0)))
                .unwrap()
                .close_plan()
                .clone(),
            ClosePlan::Tagged {
                tag_part: 0,
                arms: vec![
                    ClosePlan::Empty,
                    ClosePlan::Leaf {
                        part: 1,
                        kind: Type::ReplyToI64,
                    },
                ],
            }
        );
    }

    #[test]
    fn active_message_rights_follow_nested_tags_and_reverse_field_order() {
        let program = core::Program {
            data: vec![
                core::DataDef {
                    name: "Choice".into(),
                    variants: vec!["None".into(), "Request".into()],
                    payloads: vec![None, Some(Type::RequestI64)],
                    fields: vec![],
                },
                core::DataDef {
                    name: "Packet".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("sender".into(), Type::SenderI64),
                        ("choice".into(), Type::Data(DataId(0))),
                        ("reply".into(), Type::ReplyToI64),
                    ],
                },
            ],
            functions: vec![],
        };
        let layout = Shapes::new(&program)
            .unwrap()
            .message_layout(Type::Data(DataId(1)))
            .unwrap();
        // sender, choice tag, request value, request reply, outer reply
        let mut active = Vec::new();
        layout
            .visit_active_rights(&[11, 1, 99, 22, 33], |part, token| {
                active.push((part, token));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            active,
            [
                (
                    CapabilityPart {
                        part: 4,
                        kind: Type::ReplyToI64
                    },
                    33
                ),
                (
                    CapabilityPart {
                        part: 3,
                        kind: Type::ReplyToI64
                    },
                    22
                ),
                (
                    CapabilityPart {
                        part: 0,
                        kind: Type::SenderI64
                    },
                    11
                ),
            ]
        );
        active.clear();
        layout
            .visit_active_rights(&[11, 0, 99, 22, 33], |part, token| {
                active.push((part, token));
                Ok(())
            })
            .unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].1, 33);
        assert_eq!(active[1].1, 11);
        assert!(
            layout
                .visit_active_rights(&[11, 2, 99, 22, 33], |_, _| Ok(()))
                .is_err()
        );
        assert!(
            layout
                .visit_active_rights(&[11, 1, 99, 22], |_, _| Ok(()))
                .is_err()
        );
    }
}
