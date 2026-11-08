//! Backend independent flattened shapes for Phoebe's internal calling ABI.
//!
//! A shape describes transport, not ownership or permitted operations. Two
//! types with equal shapes may share a generic body's transport convention;
//! operations that inspect, copy, or close them still need explicit evidence.

use phoebe_core::{Program, Type};

pub mod generic;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Byte,
    Word,
    Address,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    parts: Vec<Part>,
}

impl Shape {
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }
}

/// A concrete channel message's transport and ownership identity. Equal
/// transport shapes do not imply equal types or equal cleanup behavior.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageLayout {
    pub ty: Type,
    pub shape: Shape,
    pub close: ClosePlan,
}

impl MessageLayout {
    /// Visit only the active rights in structural close order. A caller may
    /// use one read-only pass to validate ownership, then a nonfailing pass
    /// to commit transfers. The successful path allocates no temporary list.
    pub fn visit_active_rights(
        &self,
        words: &[u64],
        mut visit: impl FnMut(CapabilityPart, u64) -> Result<(), String>,
    ) -> Result<(), String> {
        if words.len() != self.shape.parts.len() {
            return Err("message word count does not match its ABI shape".into());
        }
        self.close.visit_active(words, &mut visit)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityPart {
    pub part: usize,
    pub kind: Type,
}

/// Structural close instructions over flattened ABI parts. Product fields
/// appear in reverse declaration order; a tagged arm runs only for its tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClosePlan {
    Empty,
    Leaf {
        part: usize,
        kind: Type,
    },
    Sequence(Vec<ClosePlan>),
    Tagged {
        tag_part: usize,
        arms: Vec<ClosePlan>,
    },
}

impl ClosePlan {
    fn visit_active(
        &self,
        words: &[u64],
        visit: &mut impl FnMut(CapabilityPart, u64) -> Result<(), String>,
    ) -> Result<(), String> {
        match self {
            Self::Empty => Ok(()),
            Self::Leaf { part, kind } => {
                let token = *words
                    .get(*part)
                    .ok_or("message capability part is missing")?;
                visit(
                    CapabilityPart {
                        part: *part,
                        kind: *kind,
                    },
                    token,
                )
            }
            Self::Sequence(plans) => {
                for plan in plans {
                    plan.visit_active(words, visit)?;
                }
                Ok(())
            }
            Self::Tagged { tag_part, arms } => {
                let tag = *words.get(*tag_part).ok_or("message tag part is missing")?;
                let arm = usize::try_from(tag)
                    .ok()
                    .and_then(|index| arms.get(index))
                    .ok_or("message variant tag is out of range")?;
                arm.visit_active(words, visit)
            }
        }
    }

    fn shifted(&self, offset: usize) -> Self {
        match self {
            Self::Empty => Self::Empty,
            Self::Leaf { part, kind } => Self::Leaf {
                part: part + offset,
                kind: *kind,
            },
            Self::Sequence(plans) => {
                Self::Sequence(plans.iter().map(|plan| plan.shifted(offset)).collect())
            }
            Self::Tagged { tag_part, arms } => Self::Tagged {
                tag_part: tag_part + offset,
                arms: arms.iter().map(|plan| plan.shifted(offset)).collect(),
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }
}

pub struct Shapes {
    data: Vec<Shape>,
    capabilities: Vec<Vec<CapabilityPart>>,
    closes: Vec<ClosePlan>,
    sendable: Vec<bool>,
}

impl Shapes {
    /// Data declarations must be finite and ordered before their users, as
    /// the checked Core verifier requires. Invalid input returns an error.
    pub fn new(program: &Program) -> Result<Self, String> {
        let mut shapes = Self {
            data: Vec::with_capacity(program.data.len()),
            capabilities: Vec::with_capacity(program.data.len()),
            closes: Vec::with_capacity(program.data.len()),
            sendable: Vec::with_capacity(program.data.len()),
        };
        for definition in &program.data {
            if definition.fields.is_empty() == definition.variants.is_empty() {
                return Err(format!(
                    "data `{}` is neither a sum nor a product",
                    definition.name
                ));
            }
            let mut capability_parts = Vec::new();
            let sendable = if !definition.fields.is_empty() {
                definition
                    .fields
                    .iter()
                    .all(|(_, ty)| shapes.is_sendable(*ty))
            } else {
                definition
                    .payloads
                    .iter()
                    .flatten()
                    .all(|ty| shapes.is_sendable(*ty))
            };
            let (parts, close) = if !definition.fields.is_empty() {
                let mut parts = Vec::new();
                let mut close_fields = Vec::new();
                for (_, ty) in &definition.fields {
                    let close = shapes.close_plan(*ty)?.shifted(parts.len());
                    if !close.is_empty() {
                        close_fields.push(close);
                    }
                    for leaf in shapes.capability_parts(*ty)? {
                        capability_parts.push(CapabilityPart {
                            part: parts.len() + leaf.part,
                            kind: leaf.kind,
                        });
                    }
                    parts.extend_from_slice(shapes.parts(*ty)?);
                }
                close_fields.reverse();
                (parts, Self::sequence(close_fields))
            } else {
                if definition.payloads.len() != definition.variants.len() {
                    return Err(format!(
                        "data `{}` has mismatched variants and payloads",
                        definition.name
                    ));
                }
                let mut widest = 0;
                for payload in definition.payloads.iter().flatten() {
                    widest = widest.max(shapes.parts(*payload)?.len());
                }
                let count = widest.checked_add(1).ok_or("sum ABI shape overflow")?;
                let arms = definition
                    .payloads
                    .iter()
                    .map(|payload| match payload {
                        Some(ty) => shapes.close_plan(*ty).map(|plan| plan.shifted(1)),
                        None => Ok(ClosePlan::Empty),
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let close = if arms.iter().any(|arm| !arm.is_empty()) {
                    ClosePlan::Tagged { tag_part: 0, arms }
                } else {
                    ClosePlan::Empty
                };
                (vec![Part::Word; count], close)
            };
            shapes.data.push(Shape { parts });
            shapes.capabilities.push(capability_parts);
            shapes.closes.push(close);
            shapes.sendable.push(sendable);
        }
        Ok(shapes)
    }

    fn is_sendable(&self, ty: Type) -> bool {
        match ty {
            Type::RefI64
            | Type::RefMutI64
            | Type::RefBool
            | Type::RefMutBool
            | Type::ReplyReceiverI64 => false,
            Type::Data(id) => self.sendable.get(id.0).copied().unwrap_or(false),
            _ => true,
        }
    }

    /// Build a descriptor only for a closed or transferable concrete value.
    /// The native runtime must additionally validate active rights before
    /// publishing a queue slot; this is a static layout, not a runtime proof.
    pub fn message_layout(&self, ty: Type) -> Result<MessageLayout, String> {
        if !self.is_sendable(ty) {
            return Err("message type is not sendable in the current ABI subset".into());
        }
        Ok(MessageLayout {
            ty,
            shape: self.shape(ty)?,
            close: self.close_plan(ty)?,
        })
    }

    pub fn shape(&self, ty: Type) -> Result<Shape, String> {
        Ok(Shape {
            parts: self.parts(ty)?.to_vec(),
        })
    }

    fn sequence(mut plans: Vec<ClosePlan>) -> ClosePlan {
        match plans.len() {
            0 => ClosePlan::Empty,
            1 => plans.pop().expect("one close plan"),
            _ => ClosePlan::Sequence(plans),
        }
    }

    pub fn close_plan(&self, ty: Type) -> Result<ClosePlan, String> {
        match ty {
            Type::Data(id) => self
                .closes
                .get(id.0)
                .cloned()
                .ok_or("unknown ABI data type".into()),
            Type::SenderI64
            | Type::ReceiverI64
            | Type::ReplyToI64
            | Type::ReplyReceiverI64
            | Type::RequestSenderI64
            | Type::RequestReceiverI64 => Ok(ClosePlan::Leaf { part: 0, kind: ty }),
            Type::RequestI64 => Ok(ClosePlan::Leaf {
                part: 1,
                kind: Type::ReplyToI64,
            }),
            _ => Ok(ClosePlan::Empty),
        }
    }

    pub fn parts(&self, ty: Type) -> Result<&[Part], String> {
        const WORD: &[Part] = &[Part::Word];
        const BYTE: &[Part] = &[Part::Byte];
        const EMPTY: &[Part] = &[];
        const ADDRESS: &[Part] = &[Part::Address];
        const REQUEST: &[Part] = &[Part::Word, Part::Word];
        match ty {
            Type::I64 => Ok(WORD),
            Type::Bool => Ok(BYTE),
            Type::Unit => Ok(EMPTY),
            Type::Data(id) => self
                .data
                .get(id.0)
                .map(Shape::parts)
                .ok_or("unknown ABI data type".into()),
            Type::RefI64 | Type::RefMutI64 | Type::RefBool | Type::RefMutBool => Ok(ADDRESS),
            Type::SenderI64
            | Type::ReceiverI64
            | Type::ReplyToI64
            | Type::ReplyReceiverI64
            | Type::RequestSenderI64
            | Type::RequestReceiverI64
            | Type::ResourceError => Ok(WORD),
            Type::RequestI64 => Ok(REQUEST),
        }
    }

    /// Declaration-order capability leaves with offsets in the flattened
    /// transport shape. Reverse iteration gives structural close order.
    pub fn capability_parts(&self, ty: Type) -> Result<&[CapabilityPart], String> {
        match ty {
            Type::Data(id) => self
                .capabilities
                .get(id.0)
                .map(Vec::as_slice)
                .ok_or("unknown ABI data type".into()),
            Type::SenderI64 => Ok(&[CapabilityPart {
                part: 0,
                kind: Type::SenderI64,
            }]),
            Type::ReceiverI64 => Ok(&[CapabilityPart {
                part: 0,
                kind: Type::ReceiverI64,
            }]),
            Type::ReplyToI64 => Ok(&[CapabilityPart {
                part: 0,
                kind: Type::ReplyToI64,
            }]),
            Type::ReplyReceiverI64 => Ok(&[CapabilityPart {
                part: 0,
                kind: Type::ReplyReceiverI64,
            }]),
            Type::RequestSenderI64 => Ok(&[CapabilityPart {
                part: 0,
                kind: Type::RequestSenderI64,
            }]),
            Type::RequestReceiverI64 => Ok(&[CapabilityPart {
                part: 0,
                kind: Type::RequestReceiverI64,
            }]),
            Type::RequestI64 => Ok(&[CapabilityPart {
                part: 1,
                kind: Type::ReplyToI64,
            }]),
            _ => Ok(&[]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoebe_core::{self as core, DataId};

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
            shapes.capability_parts(Type::Data(DataId(1))).unwrap(),
            [
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
        assert_eq!(closed.shape, right.shape);
        assert_ne!(closed.ty, right.ty);
        assert_eq!(closed.close, ClosePlan::Empty);
        assert_eq!(
            right.close,
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
            shapes.message_layout(Type::Unit).unwrap().shape.parts(),
            &[]
        );
        assert_eq!(
            shapes.message_layout(Type::Data(DataId(0))).unwrap().close,
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
