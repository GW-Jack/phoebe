//! Build child-spawn descriptors from the same structural ownership plan used
//! for close. A tagged plan writes only the selected payload's rights.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, Value, types,
};
use cranelift_frontend::FunctionBuilder;
use phoebe_abi::ClosePlan;
use phoebe_core::Type;

use super::codegen::AbiShapes;

fn kind(ty: Type) -> Result<i64, String> {
    match ty {
        Type::SenderI64 | Type::ReceiverI64 | Type::RequestSenderI64 | Type::RequestReceiverI64 => {
            Ok(0)
        }
        Type::ReplyToI64 => Ok(1),
        Type::ReplyReceiverI64 => Ok(2),
        _ => Err("invalid spawn transfer plan leaf".into()),
    }
}

fn has_tag(plan: &ClosePlan) -> bool {
    match plan {
        ClosePlan::Empty | ClosePlan::Leaf { .. } => false,
        ClosePlan::Sequence(plans) => plans.iter().any(has_tag),
        ClosePlan::Tagged { .. } => true,
    }
}

fn max_leaves(plan: &ClosePlan) -> Result<usize, String> {
    match plan {
        ClosePlan::Empty => Ok(0),
        ClosePlan::Leaf { .. } => Ok(1),
        ClosePlan::Sequence(plans) => plans.iter().try_fold(0usize, |count, plan| {
            count
                .checked_add(max_leaves(plan)?)
                .ok_or("spawn transfer count overflow".into())
        }),
        ClosePlan::Tagged { arms, .. } => arms
            .iter()
            .try_fold(0usize, |maximum, arm| -> Result<usize, String> {
                Ok(maximum.max(max_leaves(arm)?))
            }),
    }
}

fn emit_plan(
    builder: &mut FunctionBuilder<'_>,
    plan: &ClosePlan,
    parts: &[Value],
    cursor: Value,
    pointer: cranelift_codegen::ir::Type,
) -> Result<Value, String> {
    match plan {
        ClosePlan::Empty => Ok(cursor),
        ClosePlan::Leaf { part, kind: ty } => {
            let token = *parts
                .get(*part)
                .ok_or("spawn transfer plan part exceeds ABI shape")?;
            let kind = builder.ins().iconst(types::I64, kind(*ty)?);
            builder.ins().store(MemFlagsData::new(), kind, cursor, 0);
            builder.ins().store(MemFlagsData::new(), token, cursor, 8);
            Ok(builder.ins().iadd_imm_u(cursor, 16))
        }
        ClosePlan::Sequence(plans) => {
            let mut cursor = cursor;
            for plan in plans {
                cursor = emit_plan(builder, plan, parts, cursor, pointer)?;
            }
            Ok(cursor)
        }
        ClosePlan::Tagged { tag_part, arms } => {
            if let [only] = arms.as_slice() {
                return emit_plan(builder, only, parts, cursor, pointer);
            }
            let tag = *parts
                .get(*tag_part)
                .ok_or("spawn transfer plan tag exceeds ABI shape")?;
            let after = builder.create_block();
            builder.append_block_param(after, pointer);
            for (index, arm) in arms.iter().enumerate() {
                let selected = builder.create_block();
                let next = if index + 1 == arms.len() {
                    builder.ins().jump(selected, &[]);
                    None
                } else {
                    let next = builder.create_block();
                    let equal = builder.ins().icmp_imm_s(IntCC::Equal, tag, index as i64);
                    builder.ins().brif(equal, selected, &[], next, &[]);
                    Some(next)
                };
                builder.switch_to_block(selected);
                let selected_cursor = emit_plan(builder, arm, parts, cursor, pointer)?;
                builder.ins().jump(after, &[selected_cursor.into()]);
                if let Some(next) = next {
                    builder.switch_to_block(next);
                }
            }
            builder.switch_to_block(after);
            Ok(builder.block_params(after)[0])
        }
    }
}

/// The flat path preserves constant descriptor offsets and count. Tagged
/// arguments use one stack array sized for the largest possible variant.
pub(super) fn emit(
    builder: &mut FunctionBuilder<'_>,
    shapes: &AbiShapes,
    args: &[(Type, Vec<Value>)],
    pointer: cranelift_codegen::ir::Type,
) -> Result<(Value, Value), String> {
    let plans = args
        .iter()
        .map(|(ty, _)| shapes.close_plan(*ty))
        .collect::<Vec<_>>();
    if !plans.iter().any(has_tag) {
        let mut transfers = Vec::new();
        for (ty, parts) in args {
            shapes.append_transfer_descriptors(*ty, &mut transfers, |part| parts[part]);
        }
        let count = i64::try_from(transfers.len()).map_err(|_| "spawn transfer count overflow")?;
        if transfers.is_empty() {
            let null = builder.ins().iconst(pointer, 0);
            let count = builder.ins().iconst(types::I64, 0);
            return Ok((null, count));
        }
        let bytes = stack_bytes(transfers.len())?;
        let slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            bytes,
            3,
        ));
        for (index, (kind, token)) in transfers.into_iter().enumerate() {
            let kind = builder.ins().iconst(types::I64, kind);
            let offset = i32::try_from(index * 16).map_err(|_| "spawn transfer offset overflow")?;
            builder.ins().stack_store(pointer, kind, slot, offset);
            builder.ins().stack_store(pointer, token, slot, offset + 8);
        }
        let base = builder.ins().stack_addr(pointer, slot, 0);
        let count = builder.ins().iconst(types::I64, count);
        return Ok((base, count));
    }

    let maximum = plans.iter().try_fold(0usize, |count, plan| {
        count
            .checked_add(max_leaves(plan)?)
            .ok_or_else(|| "spawn transfer count overflow".to_string())
    })?;
    let bytes = stack_bytes(maximum)?;
    let slot =
        builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 3));
    let base = builder.ins().stack_addr(pointer, slot, 0);
    let mut cursor = base;
    for (plan, (_, parts)) in plans.iter().zip(args) {
        cursor = emit_plan(builder, plan, parts, cursor, pointer)?;
    }
    let bytes = builder.ins().isub(cursor, base);
    let count = builder.ins().ushr_imm_u(bytes, 4);
    Ok((base, count))
}

fn stack_bytes(count: usize) -> Result<u32, String> {
    count
        .checked_mul(16)
        .filter(|bytes| *bytes <= i32::MAX as usize)
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or("spawn transfer list exceeds stack slot".into())
}
