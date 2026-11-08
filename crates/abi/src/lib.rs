//! Backend independent flattened shapes for Phoebe's internal calling ABI.
//!
//! A shape describes transport, not ownership or permitted operations. Two
//! types with equal shapes may share a generic body's transport convention;
//! operations that inspect, copy, or close them still need explicit evidence.

use phoebe_core::{Program, Type};

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

pub struct Shapes {
    data: Vec<Shape>,
}

impl Shapes {
    /// Data declarations must be finite and ordered before their users, as
    /// the checked Core verifier requires. Invalid input returns an error.
    pub fn new(program: &Program) -> Result<Self, String> {
        let mut shapes = Self {
            data: Vec::with_capacity(program.data.len()),
        };
        for definition in &program.data {
            if definition.fields.is_empty() == definition.variants.is_empty() {
                return Err(format!(
                    "data `{}` is neither a sum nor a product",
                    definition.name
                ));
            }
            let parts = if !definition.fields.is_empty() {
                let mut parts = Vec::new();
                for (_, ty) in &definition.fields {
                    parts.extend_from_slice(shapes.parts(*ty)?);
                }
                parts
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
                vec![Part::Word; count]
            };
            shapes.data.push(Shape { parts });
        }
        Ok(shapes)
    }

    pub fn shape(&self, ty: Type) -> Result<Shape, String> {
        Ok(Shape {
            parts: self.parts(ty)?.to_vec(),
        })
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
            Type::RefI64 | Type::RefMutI64 => Ok(ADDRESS),
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
}
