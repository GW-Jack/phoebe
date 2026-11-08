//! Turn exact live-slot facts into source cleanup operations.

use super::*;
use std::collections::HashMap;

pub(super) fn close_operation(value: ValueId, ty: Type) -> Operation {
    match ty {
        Type::Data(_) => Operation::CloseCaps(value),
        Type::ReplyToI64 | Type::ReplyReceiverI64 => Operation::CloseReply(value),
        Type::RequestI64 => Operation::CloseRequest(value),
        _ => Operation::CloseHandle(value),
    }
}

pub(super) fn close_live_owners(
    value_types: &[Type],
    layout: &verification::capability_layout::Layout,
    slot_owners: &[ValueId],
    live: &[capability_flow::Slot],
) -> Result<Vec<Operation>, String> {
    let mut owners = Vec::new();
    let mut counts = HashMap::new();
    for slot in live {
        let value = *slot_owners
            .get(slot.0)
            .ok_or_else(|| "invalid live capability slot".to_owned())?;
        let count = counts.entry(value.0).or_insert_with(|| {
            owners.push(value);
            0usize
        });
        *count += 1;
    }
    owners
        .into_iter()
        .map(|value| {
            let ty = value_types[value.0];
            if counts[&value.0] != layout.leaves(ty) {
                return Err(format!(
                    "partial ownership of capability product v{} at cleanup",
                    value.0
                ));
            }
            Ok(close_operation(value, ty))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_product_owner_gets_one_structural_cleanup() {
        let data = vec![DataDef {
            name: "Pair".into(),
            variants: vec![],
            payloads: vec![],
            fields: vec![
                ("sender".into(), Type::SenderI64),
                ("reply".into(), Type::ReplyToI64),
            ],
        }];
        let layout = verification::capability_layout::Layout::new(&data);
        let types = [Type::Data(DataId(0)), Type::ReceiverI64];
        let owners = [ValueId(0), ValueId(0), ValueId(1)];
        let live = [
            capability_flow::Slot(0),
            capability_flow::Slot(1),
            capability_flow::Slot(2),
        ];
        assert!(matches!(
            close_live_owners(&types, &layout, &owners, &live)
                .unwrap()
                .as_slice(),
            [
                Operation::CloseCaps(ValueId(0)),
                Operation::CloseHandle(ValueId(1))
            ]
        ));
        assert!(
            close_live_owners(&types, &layout, &owners, &live[..1])
                .unwrap_err()
                .contains("partial ownership")
        );
    }
}
