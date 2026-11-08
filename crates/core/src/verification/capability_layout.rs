//! Static capability leaves of acyclic nominal products. Data declarations
//! are ordered, so a field can refer only to an earlier data layout.

use super::{DataDef, DataId, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Leaf {
    /// Declaration-order field indices from the root value to this right.
    pub(super) path: Vec<usize>,
    pub(super) kind: Type,
}

pub(crate) struct Layout {
    products: Vec<Vec<Leaf>>,
}

impl Layout {
    pub(crate) fn new(data: &[DataDef]) -> Self {
        let mut layout = Self {
            products: Vec::with_capacity(data.len()),
        };
        for definition in data {
            let mut leaves = Vec::new();
            // Sum payloads require a tag-sensitive state and remain rejected
            // by structural verification. Only products have static paths.
            for (field, (_, ty)) in definition.fields.iter().enumerate() {
                match ty {
                    Type::Data(DataId(id)) => {
                        for nested in &layout.products[*id] {
                            let mut path = Vec::with_capacity(nested.path.len() + 1);
                            path.push(field);
                            path.extend_from_slice(&nested.path);
                            leaves.push(Leaf {
                                path,
                                kind: nested.kind,
                            });
                        }
                    }
                    ty if Self::bare_right(*ty) => leaves.push(Leaf {
                        path: vec![field],
                        kind: *ty,
                    }),
                    _ => {}
                }
            }
            layout.products.push(leaves);
        }
        layout
    }

    pub(super) fn bare_right(ty: Type) -> bool {
        matches!(
            ty,
            Type::SenderI64
                | Type::ReceiverI64
                | Type::ReplyToI64
                | Type::ReplyReceiverI64
                | Type::RequestI64
                | Type::RequestSenderI64
                | Type::RequestReceiverI64
        )
    }

    pub(super) fn leaves(&self, ty: Type) -> usize {
        match ty {
            Type::Data(DataId(id)) => self.products[id].len(),
            ty if Self::bare_right(ty) => 1,
            _ => 0,
        }
    }

    #[cfg(test)]
    fn product(&self, id: DataId) -> &[Leaf] {
        &self.products[id.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_products_have_distinct_stable_capability_paths() {
        let layout = Layout::new(&[
            DataDef {
                name: "Pair".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![
                    ("left".into(), Type::SenderI64),
                    ("empty".into(), Type::Unit),
                    ("right".into(), Type::ReplyToI64),
                ],
            },
            DataDef {
                name: "Envelope".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![
                    ("n".into(), Type::I64),
                    ("pair".into(), Type::Data(DataId(0))),
                    ("request".into(), Type::RequestI64),
                ],
            },
        ]);
        assert_eq!(layout.leaves(Type::Data(DataId(1))), 3);
        assert_eq!(
            layout.product(DataId(1)),
            [
                Leaf {
                    path: vec![1, 0],
                    kind: Type::SenderI64
                },
                Leaf {
                    path: vec![1, 2],
                    kind: Type::ReplyToI64
                },
                Leaf {
                    path: vec![2],
                    kind: Type::RequestI64
                },
            ]
        );
    }
}
