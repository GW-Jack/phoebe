//! Shared Cranelift materialization of typed queue descriptors and words.

use super::*;
use cranelift_codegen::ir::StackSlot;
use cranelift_module::{DataDescription, DataId as ModuleDataId};
use std::collections::HashMap;

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

/// One immutable copy per distinct plan in a native module. A channel
/// creation site only materializes its address and length, never its words.
#[derive(Default)]
pub(super) struct PlanData {
    ids: HashMap<Vec<u64>, ModuleDataId>,
}

impl PlanData {
    pub(super) fn pointer<M: Module>(
        &mut self,
        module: &mut M,
        builder: &mut FunctionBuilder<'_>,
        pointer: cranelift_codegen::ir::Type,
        descriptor: &phoebe_message::RuntimeMessageDescriptor,
    ) -> Result<(ClValue, ClValue), String> {
        let words = descriptor.rights.encode_native_words();
        if words.len() > runtime::NATIVE_TYPED_PLAN_MAX_WORDS {
            return Err("typed message right plan exceeds native ABI limit".into());
        }
        let length = words.len();
        let id = if let Some(id) = self.ids.get(&words) {
            *id
        } else {
            let id = module
                .declare_anonymous_data(false, false)
                .map_err(|error| error.to_string())?;
            let mut data = DataDescription::new();
            data.define(
                words
                    .iter()
                    .flat_map(|word| word.to_ne_bytes())
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            data.set_align(8);
            module
                .define_data(id, &data)
                .map_err(|error| error.to_string())?;
            self.ids.insert(words, id);
            id
        };
        let global = module.declare_data_in_func(id, builder.func);
        let address = builder.ins().symbol_value(pointer, global);
        let length = builder.ins().iconst(types::I64, length as i64);
        Ok((address, length))
    }
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
