//! Shared concrete types and flattened message layouts.
//! Core and native code use this one definition of transport and cleanup.

use phoebe_syntax::syntax::ParamMode;
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DataId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageType {
    I64,
    Bool,
    Unit,
    Data(DataId),
    RequestI64,
    ReplyToI64,
    ResourceError,
}

impl MessageType {
    pub fn value_type(self) -> Type {
        match self {
            Self::I64 => Type::I64,
            Self::Bool => Type::Bool,
            Self::Unit => Type::Unit,
            Self::Data(id) => Type::Data(id),
            Self::RequestI64 => Type::RequestI64,
            Self::ReplyToI64 => Type::ReplyToI64,
            Self::ResourceError => Type::ResourceError,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    I64,
    Bool,
    Unit,
    /// An inline, fixed-size scalar array. Larger arrays need region storage.
    ArrayI64(usize),
    Data(DataId),
    RefI64,
    RefMutI64,
    RefBool,
    RefMutBool,
    SenderI64,
    ReceiverI64,
    ReplyToI64,
    ReplyReceiverI64,
    RequestI64,
    RequestSenderI64,
    RequestReceiverI64,
    TypedSender(MessageType),
    TypedReceiver(MessageType),
    ResourceError,
}

impl Type {
    pub fn parameter(mode: ParamMode, owner: Self) -> Option<Self> {
        match (mode, owner) {
            (ParamMode::Own, owner) => Some(owner),
            (ParamMode::Shared, Self::I64) => Some(Self::RefI64),
            (ParamMode::Exclusive, Self::I64) => Some(Self::RefMutI64),
            (ParamMode::Shared, Self::Bool) => Some(Self::RefBool),
            (ParamMode::Exclusive, Self::Bool) => Some(Self::RefMutBool),
            _ => None,
        }
    }

    pub fn referent(self) -> Option<Self> {
        match self {
            Self::RefI64 | Self::RefMutI64 => Some(Self::I64),
            Self::RefBool | Self::RefMutBool => Some(Self::Bool),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataDef {
    pub name: String,
    pub variants: Vec<String>,
    pub payloads: Vec<Option<Type>>,
    pub fields: Vec<(String, Type)>,
}

pub trait DataDefinitions {
    fn definitions(&self) -> &[DataDef];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Byte,
    Word,
    Address,
}

const INLINE_ARRAY_WORDS: [Part; 32] = [Part::Word; 32];

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
    ty: Type,
    shape: Shape,
    close: ClosePlan,
}

impl MessageLayout {
    pub fn ty(&self) -> Type {
        self.ty
    }

    pub fn shape(&self) -> &Shape {
        &self.shape
    }

    pub fn close_plan(&self) -> &ClosePlan {
        &self.close
    }

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
    pub fn new(program: &impl DataDefinitions) -> Result<Self, String> {
        let mut shapes = Self {
            data: Vec::with_capacity(program.definitions().len()),
            capabilities: Vec::with_capacity(program.definitions().len()),
            closes: Vec::with_capacity(program.definitions().len()),
            sendable: Vec::with_capacity(program.definitions().len()),
        };
        for definition in program.definitions() {
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
                    for leaf in shapes.capability_parts(*ty)?.iter() {
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
            | Type::ReplyReceiverI64
            | Type::ArrayI64(_) => false,
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
            | Type::RequestReceiverI64
            | Type::TypedSender(_)
            | Type::TypedReceiver(_) => Ok(ClosePlan::Leaf { part: 0, kind: ty }),
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
            Type::ArrayI64(len) if len <= 32 => Ok(&INLINE_ARRAY_WORDS[..len]),
            Type::ArrayI64(_) => Err("inline I64 array exceeds 32 elements".into()),
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
            | Type::TypedSender(_)
            | Type::TypedReceiver(_)
            | Type::ResourceError => Ok(WORD),
            Type::RequestI64 => Ok(REQUEST),
        }
    }

    /// Declaration-order capability leaves with offsets in the flattened
    /// transport shape. Reverse iteration gives structural close order.
    pub fn capability_parts(&self, ty: Type) -> Result<Cow<'_, [CapabilityPart]>, String> {
        match ty {
            Type::Data(id) => self
                .capabilities
                .get(id.0)
                .map(|parts| Cow::Borrowed(parts.as_slice()))
                .ok_or("unknown ABI data type".into()),
            Type::SenderI64 => Ok(Cow::Borrowed(&[CapabilityPart {
                part: 0,
                kind: Type::SenderI64,
            }])),
            Type::ReceiverI64 => Ok(Cow::Borrowed(&[CapabilityPart {
                part: 0,
                kind: Type::ReceiverI64,
            }])),
            Type::ReplyToI64 => Ok(Cow::Borrowed(&[CapabilityPart {
                part: 0,
                kind: Type::ReplyToI64,
            }])),
            Type::ReplyReceiverI64 => Ok(Cow::Borrowed(&[CapabilityPart {
                part: 0,
                kind: Type::ReplyReceiverI64,
            }])),
            Type::RequestSenderI64 => Ok(Cow::Borrowed(&[CapabilityPart {
                part: 0,
                kind: Type::RequestSenderI64,
            }])),
            Type::RequestReceiverI64 => Ok(Cow::Borrowed(&[CapabilityPart {
                part: 0,
                kind: Type::RequestReceiverI64,
            }])),
            Type::TypedSender(_) | Type::TypedReceiver(_) => {
                Ok(Cow::Owned(vec![CapabilityPart { part: 0, kind: ty }]))
            }
            Type::RequestI64 => Ok(Cow::Borrowed(&[CapabilityPart {
                part: 1,
                kind: Type::ReplyToI64,
            }])),
            _ => Ok(Cow::Borrowed(&[])),
        }
    }
}
