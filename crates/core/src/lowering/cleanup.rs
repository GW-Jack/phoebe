//! Turn exact live-slot facts into source cleanup operations.

use super::*;
use std::collections::HashMap;

#[derive(Debug)]
pub(super) enum LiveOwner {
    Whole(verification::SlotOwner),
    PlaceField(PlaceId, usize),
}

pub(super) fn close_operation(value: ValueId, ty: Type) -> Operation {
    match ty {
        Type::Data(_) => Operation::CloseCaps(value),
        Type::ReplyToI64 | Type::ReplyReceiverI64 => Operation::CloseReply(value),
        Type::RequestI64 => Operation::CloseRequest(value),
        _ => Operation::CloseHandle(value),
    }
}

pub(super) fn live_owners(
    value_types: &[Type],
    place_types: &[Type],
    layout: &verification::capability_layout::Layout,
    slot_owners: &[verification::SlotOwner],
    live: &[capability_flow::Slot],
) -> Result<Vec<LiveOwner>, String> {
    let mut owners = Vec::new();
    let mut counts = HashMap::new();
    for slot in live {
        let owner = *slot_owners
            .get(slot.0)
            .ok_or_else(|| "invalid live capability slot".to_owned())?;
        let count = counts.entry(owner).or_insert_with(|| {
            owners.push(owner);
            0usize
        });
        *count += 1;
    }
    let live_slots = live
        .iter()
        .map(|slot| slot.0)
        .collect::<std::collections::HashSet<_>>();
    let mut cleanup = Vec::new();
    for owner in owners {
        let ty = match owner {
            verification::SlotOwner::Value(value) => value_types[value.0],
            verification::SlotOwner::Place(place) => place_types[place.0],
        };
        if counts[&owner] == layout.leaves(ty) {
            cleanup.push(LiveOwner::Whole(owner));
            continue;
        }
        let (verification::SlotOwner::Place(place), Type::Data(id)) = (owner, ty) else {
            return Err(format!(
                "partial ownership of capability owner {owner:?} at cleanup"
            ));
        };
        let base = slot_owners
            .iter()
            .position(|candidate| *candidate == owner)
            .ok_or("live place has no capability slots")?;
        for field in (0..layout.field_count(id)).rev() {
            let leaves = layout.field_leaves(id, field);
            let width = leaves.len();
            if width == 0 {
                continue;
            }
            let present = (leaves.start..leaves.end)
                .filter(|offset| live_slots.contains(&(base + offset)))
                .count();
            if present == width {
                cleanup.push(LiveOwner::PlaceField(place, field));
            } else if present != 0 {
                return Err(format!(
                    "partial ownership inside field {field} of p{} at cleanup",
                    place.0
                ));
            }
        }
    }
    Ok(cleanup)
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
        let owners = [
            verification::SlotOwner::Value(ValueId(0)),
            verification::SlotOwner::Value(ValueId(0)),
            verification::SlotOwner::Value(ValueId(1)),
        ];
        let live = [
            capability_flow::Slot(0),
            capability_flow::Slot(1),
            capability_flow::Slot(2),
        ];
        assert!(matches!(
            live_owners(&types, &[], &layout, &owners, &live)
                .unwrap()
                .as_slice(),
            [
                LiveOwner::Whole(verification::SlotOwner::Value(ValueId(0))),
                LiveOwner::Whole(verification::SlotOwner::Value(ValueId(1)))
            ]
        ));
        assert!(
            live_owners(&types, &[], &layout, &owners, &live[..1])
                .unwrap_err()
                .contains("partial ownership")
        );
    }
}
