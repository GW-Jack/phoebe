//! Emit the same structural close plan for direct and retained functions.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{InstBuilder, Value};
use cranelift_frontend::FunctionBuilder;
use phoebe_abi::ClosePlan;
use phoebe_core::Type;

pub(super) fn emit<F>(
    builder: &mut FunctionBuilder<'_>,
    plan: &ClosePlan,
    parts: &[Value],
    close_leaf: &mut F,
) -> Result<(), String>
where
    F: FnMut(&mut FunctionBuilder<'_>, Type, Value) -> Result<(), String>,
{
    match plan {
        ClosePlan::Empty => {}
        ClosePlan::Leaf { part, kind } => {
            let token = *parts
                .get(*part)
                .ok_or("close plan part exceeds ABI shape")?;
            close_leaf(builder, *kind, token)?;
        }
        ClosePlan::Sequence(plans) => {
            for plan in plans {
                emit(builder, plan, parts, close_leaf)?;
            }
        }
        ClosePlan::Tagged { tag_part, arms } => {
            if let [only] = arms.as_slice() {
                return emit(builder, only, parts, close_leaf);
            }
            let tag = *parts
                .get(*tag_part)
                .ok_or("close plan tag exceeds ABI shape")?;
            let after = builder.create_block();
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
                emit(builder, arm, parts, close_leaf)?;
                builder.ins().jump(after, &[]);
                if let Some(next) = next {
                    builder.switch_to_block(next);
                }
            }
            builder.switch_to_block(after);
        }
    }
    Ok(())
}
