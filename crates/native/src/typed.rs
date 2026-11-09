//! Shared Cranelift materialization of typed queue descriptors and words.

use super::*;
use cranelift_codegen::ir::StackSlot;

fn slot_bytes(words: usize) -> Result<u32, String> {
    words
        .max(1)
        .checked_mul(8)
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or("typed message stack slot exceeds native limit".into())
}

fn offset(part: usize) -> Result<i32, String> {
    part.checked_mul(8)
        .and_then(|bytes| i32::try_from(bytes).ok())
        .ok_or("typed message part offset exceeds native limit".into())
}

pub(super) fn plan_pointer(
    builder: &mut FunctionBuilder<'_>,
    pointer: cranelift_codegen::ir::Type,
    descriptor: &phoebe_message::RuntimeMessageDescriptor,
) -> Result<(ClValue, ClValue), String> {
    let words = descriptor.rights.encode_native_words();
    if words.len() > runtime::NATIVE_TYPED_PLAN_MAX_WORDS {
        return Err("typed message right plan exceeds native ABI limit".into());
    }
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        slot_bytes(words.len())?,
        3,
    ));
    for (part, word) in words.iter().enumerate() {
        let value = builder.ins().iconst(types::I64, *word as i64);
        builder
            .ins()
            .stack_store(pointer, value, slot, offset(part)?);
    }
    let address = builder.ins().stack_addr(pointer, slot, 0);
    let length = builder.ins().iconst(types::I64, words.len() as i64);
    Ok((address, length))
}

pub(super) fn input_words(
    builder: &mut FunctionBuilder<'_>,
    pointer: cranelift_codegen::ir::Type,
    values: &[ClValue],
) -> Result<ClValue, String> {
    if values.is_empty() {
        return Ok(builder.ins().iconst(pointer, 0));
    }
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        slot_bytes(values.len())?,
        3,
    ));
    for (part, value) in values.iter().enumerate() {
        let word = if builder.func.dfg.value_type(*value) == types::I8 {
            builder.ins().uextend(types::I64, *value)
        } else {
            *value
        };
        builder
            .ins()
            .stack_store(pointer, word, slot, offset(part)?);
    }
    Ok(builder.ins().stack_addr(pointer, slot, 0))
}

pub(super) fn output_words(
    builder: &mut FunctionBuilder<'_>,
    pointer: cranelift_codegen::ir::Type,
    width: usize,
) -> Result<(ClValue, Option<StackSlot>), String> {
    if width == 0 {
        return Ok((builder.ins().iconst(pointer, 0), None));
    }
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        slot_bytes(width)?,
        3,
    ));
    Ok((builder.ins().stack_addr(pointer, slot, 0), Some(slot)))
}

pub(super) fn load_output_words(
    builder: &mut FunctionBuilder<'_>,
    pointer: cranelift_codegen::ir::Type,
    slot: Option<StackSlot>,
    types_for_parts: &[cranelift_codegen::ir::Type],
) -> Result<Vec<ClValue>, String> {
    types_for_parts
        .iter()
        .enumerate()
        .map(|(part, ty)| {
            let slot = slot.ok_or("missing typed message output slot")?;
            let raw = builder
                .ins()
                .stack_load(pointer, types::I64, slot, offset(part)?);
            Ok(if *ty == types::I8 {
                builder.ins().ireduce(types::I8, raw)
            } else {
                raw
            })
        })
        .collect()
}
