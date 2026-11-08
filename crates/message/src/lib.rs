//! Translate checked, backend-independent message layouts to the current
//! runtime's typed queue descriptor. Callers cache descriptors across sends.

use phoebe_layout::{ClosePlan, DataDefinitions, DataId, MessageLayout, Shapes, Type};
use phoebe_runtime::{REQUEST_I64_MESSAGE_TYPE_ID, TypedRightPlan};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeMessageDescriptor {
    pub type_id: u64,
    pub width: usize,
    pub rights: TypedRightPlan,
}

/// Reuse one verified program's shape table when creating channel descriptors.
/// Construction is fallible and precedes creation of a channel using it.
pub struct MessageCatalog {
    shapes: Shapes,
}

impl MessageCatalog {
    pub fn new(data: &impl DataDefinitions) -> Result<Self, String> {
        Ok(Self {
            shapes: Shapes::new(data)?,
        })
    }

    pub fn descriptor(&self, ty: Type) -> Result<RuntimeMessageDescriptor, String> {
        runtime_message_descriptor(&self.shapes.message_layout(ty)?)
    }

    pub fn shapes(&self) -> &Shapes {
        &self.shapes
    }
}

/// Type IDs are unique within one checked linked Program. The high bit is
/// reserved for its dense nominal DataId space; low values name built-ins.
/// A descriptor must never be reused across independently linked programs.
fn type_id(ty: Type) -> Result<u64, String> {
    match ty {
        Type::I64 => Ok(1),
        Type::Bool => Ok(2),
        Type::Unit => Ok(3),
        Type::RequestI64 => Ok(REQUEST_I64_MESSAGE_TYPE_ID),
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
    use phoebe_runtime::TypedRightPlan;

    struct Definitions(Vec<phoebe_layout::DataDef>);

    impl DataDefinitions for Definitions {
        fn definitions(&self) -> &[phoebe_layout::DataDef] {
            &self.0
        }
    }

    #[test]
    fn catalog_distinguishes_equal_width_messages_and_active_rights() {
        use phoebe_layout::DataDef;
        let catalog = MessageCatalog::new(&Definitions(vec![
            DataDef {
                name: "Number".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![("value".into(), Type::I64)],
            },
            DataDef {
                name: "Right".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![("reply".into(), Type::ReplyToI64)],
            },
        ]))
        .unwrap();
        let number = catalog.descriptor(Type::Data(DataId(0))).unwrap();
        let right = catalog.descriptor(Type::Data(DataId(1))).unwrap();
        assert_eq!(number.width, 1);
        assert_eq!(right.width, 1);
        assert_ne!(number.type_id, right.type_id);
        assert_eq!(number.rights, TypedRightPlan::Empty);
        assert_eq!(right.rights, TypedRightPlan::ReplyTo { part: 0 });
        assert!(catalog.descriptor(Type::RefI64).is_err());
    }
}
