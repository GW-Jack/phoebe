//! Cranelift emission for the currently verified, non-suspending
//! parametric Core subset. Each planned ABI instance is defined once.

use std::fmt::Write as _;

use cranelift_codegen::ir::{
    AbiParam, ArgumentPurpose, MemFlagsData, Signature, UserFuncName, Value as ClValue, types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_native::builder as native_builder;
use cranelift_object::{ObjectBuilder, ObjectModule};
use phoebe_abi::generic::{Instance, Planner, TargetAbi};
use phoebe_abi::{Part, Shape};
use phoebe_generic_core::{self as generic_core, Operation, Terminator, Type};

use super::codegen::emit_direct_call;
use cranelift_codegen::ir::InstBuilder;

pub struct GenericJitExecutable {
    module: JITModule,
    entry: FuncId,
    clif: String,
    instance_count: usize,
}

impl GenericJitExecutable {
    pub fn clif(&self) -> &str {
        &self.clif
    }

    pub fn instance_count(&self) -> usize {
        self.instance_count
    }

    /// # Safety
    /// The caller must use the exact flattened Phoebe ABI in the selected
    /// instance signature. The executable must outlive every call.
    pub unsafe fn entry(&self) -> *const u8 {
        self.module.get_finalized_function(self.entry)
    }
}

fn native_isa(
    is_pic: bool,
) -> Result<std::sync::Arc<dyn cranelift_codegen::isa::TargetIsa>, String> {
    let mut flags = settings::builder();
    flags
        .set("use_colocated_libcalls", "false")
        .map_err(|error| error.to_string())?;
    flags
        .set("is_pic", if is_pic { "true" } else { "false" })
        .map_err(|error| error.to_string())?;
    native_builder()
        .map_err(|error| format!("unsupported native host: {error}"))?
        .finish(settings::Flags::new(flags))
        .map_err(|error| error.to_string())
}

pub fn host_target_abi() -> Result<TargetAbi, String> {
    let isa = native_isa(false)?;
    Ok(TargetAbi {
        triple: isa.triple().to_string(),
        direct_register_results: if isa.name() == "aarch64" { 8 } else { 2 },
    })
}

fn cl_type(part: Part, pointer: cranelift_codegen::ir::Type) -> cranelift_codegen::ir::Type {
    match part {
        Part::Byte => types::I8,
        Part::Word => types::I64,
        Part::Address => pointer,
    }
}

fn part_types(
    parts: &[Part],
    pointer: cranelift_codegen::ir::Type,
) -> Vec<cranelift_codegen::ir::Type> {
    parts.iter().map(|part| cl_type(*part, pointer)).collect()
}

fn value_parts(ty: Type, binders: &[Shape]) -> Result<&[Part], String> {
    const WORD: &[Part] = &[Part::Word];
    const BYTE: &[Part] = &[Part::Byte];
    match ty {
        Type::Param(index) => binders
            .get(index)
            .map(Shape::parts)
            .ok_or_else(|| format!("missing binder shape T{index}")),
        Type::I64 => Ok(WORD),
        Type::Bool => Ok(BYTE),
        Type::Unit => Ok(&[]),
    }
}

fn signature<M: Module>(module: &M, instance: &Instance) -> Signature {
    let mut signature = module.make_signature();
    let pointer = module.target_config().pointer_type();
    for shape in &instance.key.signature.params {
        signature.params.extend(
            shape
                .parts()
                .iter()
                .map(|part| AbiParam::new(cl_type(*part, pointer))),
        );
    }
    if instance.key.signature.return_buffer {
        signature
            .params
            .push(AbiParam::special(pointer, ArgumentPurpose::StructReturn));
    } else {
        signature.returns.extend(
            instance
                .key
                .signature
                .result
                .parts()
                .iter()
                .map(|part| AbiParam::new(cl_type(*part, pointer))),
        );
    }
    signature
}

fn get(values: &[Option<Vec<ClValue>>], value: generic_core::ValueId) -> Vec<ClValue> {
    values[value.0]
        .as_ref()
        .expect("verified generic value order")
        .clone()
}

/// Emit every planned instance through the same direct-call convention as
/// concrete Core. The planner has already checked bodies and call shapes.
fn emit_into<M: Module>(
    module: &mut M,
    planner: &Planner<'_>,
    entry: usize,
    export_entry: bool,
) -> Result<(Vec<FuncId>, String), String> {
    let host = TargetAbi {
        triple: module.isa().triple().to_string(),
        direct_register_results: if module.isa().name() == "aarch64" {
            8
        } else {
            2
        },
    };
    if planner.target() != &host {
        return Err("generic planner target does not match the native host ABI".into());
    }
    let instances = planner.instances();
    if entry >= instances.len() {
        return Err(format!("invalid generic entry instance {entry}"));
    }
    let pointer = module.target_config().pointer_type();
    let mut ids = Vec::with_capacity(instances.len());
    for (index, instance) in instances.iter().enumerate() {
        let signature = signature(&module, instance);
        let (symbol, linkage) = if export_entry && index == entry {
            ("phoebe_generic_entry".to_string(), Linkage::Export)
        } else {
            (format!("phoebe_generic_{index}"), Linkage::Local)
        };
        let id = module
            .declare_function(&symbol, linkage, &signature)
            .map_err(|error| error.to_string())?;
        ids.push((id, signature));
    }
    let mut context = module.make_context();
    let mut builder_context = FunctionBuilderContext::new();
    let mut clif = String::new();
    for (index, instance) in instances.iter().enumerate() {
        let function = &planner.module().functions[instance.key.function];
        context.func.signature = ids[index].1.clone();
        context.func.name = UserFuncName::user(0, ids[index].0.as_u32());
        {
            let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let blocks = function
                .blocks
                .iter()
                .map(|_| builder.create_block())
                .collect::<Vec<_>>();
            builder.append_block_params_for_function_params(blocks[0]);
            for (block_id, block) in function.blocks.iter().enumerate().skip(1) {
                for param in &block.params {
                    for part in value_parts(function.value_types[param.0], &instance.binders)? {
                        builder.append_block_param(blocks[block_id], cl_type(*part, pointer));
                    }
                }
            }
            let return_address = instance.key.signature.return_buffer.then(|| {
                *builder
                    .block_params(blocks[0])
                    .last()
                    .expect("planned return buffer")
            });
            let mut values = vec![None; function.value_types.len()];
            for (block_id, block) in function.blocks.iter().enumerate() {
                let raw = builder.block_params(blocks[block_id]);
                let mut offset = 0;
                for param in &block.params {
                    let count =
                        value_parts(function.value_types[param.0], &instance.binders)?.len();
                    values[param.0] = Some(raw[offset..offset + count].to_vec());
                    offset += count;
                }
            }
            for (block_id, block) in function.blocks.iter().enumerate() {
                builder.switch_to_block(blocks[block_id]);
                for (instruction_id, instruction) in block.instructions.iter().enumerate() {
                    let result = match &instruction.operation {
                        Operation::Move(value) | Operation::CopyScalar(value) => {
                            get(&values, *value)
                        }
                        Operation::Call { args, .. } => {
                            let target = instance
                                .calls
                                .iter()
                                .find(|call| {
                                    call.block == block_id && call.instruction == instruction_id
                                })
                                .ok_or("missing planned generic call")?
                                .target;
                            let callee = module.declare_func_in_func(ids[target].0, builder.func);
                            let arguments = args
                                .iter()
                                .flat_map(|value| get(&values, *value))
                                .collect::<Vec<_>>();
                            let result_types =
                                part_types(instances[target].key.signature.result.parts(), pointer);
                            emit_direct_call(
                                &mut builder,
                                callee,
                                arguments,
                                &result_types,
                                pointer,
                                host.direct_register_results,
                            )?
                        }
                    };
                    values[instruction.result.0] = Some(result);
                }
                match &block.terminator {
                    Terminator::Return(value) => {
                        let returned = get(&values, *value);
                        if let Some(address) = return_address {
                            for (part, (component, ty)) in returned
                                .into_iter()
                                .zip(part_types(instance.key.signature.result.parts(), pointer))
                                .enumerate()
                            {
                                let raw = if ty == types::I8 {
                                    builder.ins().uextend(types::I64, component)
                                } else {
                                    component
                                };
                                let offset = i32::try_from(part * 8)
                                    .map_err(|_| "generic return offset overflow")?;
                                builder
                                    .ins()
                                    .store(MemFlagsData::new(), raw, address, offset);
                            }
                            builder.ins().return_(&[]);
                        } else {
                            builder.ins().return_(&returned);
                        }
                    }
                    Terminator::Jump { target, args } => {
                        let arguments = args
                            .iter()
                            .flat_map(|value| get(&values, *value))
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        builder.ins().jump(blocks[target.0], &arguments);
                    }
                    Terminator::Branch {
                        condition,
                        when_true,
                        true_args,
                        when_false,
                        false_args,
                    } => {
                        let true_values = true_args
                            .iter()
                            .flat_map(|value| get(&values, *value))
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        let false_values = false_args
                            .iter()
                            .flat_map(|value| get(&values, *value))
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        builder.ins().brif(
                            get(&values, *condition)[0],
                            blocks[when_true.0],
                            &true_values,
                            blocks[when_false.0],
                            &false_values,
                        );
                    }
                }
            }
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }
        writeln!(
            clif,
            "; Phoebe generic instance {index}\n{}",
            context.func.display()
        )
        .map_err(|error| error.to_string())?;
        module
            .define_function(ids[index].0, &mut context)
            .map_err(|error| error.to_string())?;
        module.clear_context(&mut context);
    }
    Ok((ids.into_iter().map(|(id, _)| id).collect(), clif))
}

pub fn compile_jit(planner: &Planner<'_>, entry: usize) -> Result<GenericJitExecutable, String> {
    let isa = native_isa(false)?;
    let mut module = JITModule::new(JITBuilder::with_isa(isa, default_libcall_names()));
    let (ids, clif) = emit_into(&mut module, planner, entry, false)?;
    module
        .finalize_definitions()
        .map_err(|error| error.to_string())?;
    Ok(GenericJitExecutable {
        module,
        entry: ids[entry],
        clif,
        instance_count: ids.len(),
    })
}

/// Emit a linkable object with one exported `phoebe_generic_entry` symbol.
/// The object uses the same flattened calling convention as the JIT path.
pub fn compile_object(planner: &Planner<'_>, entry: usize) -> Result<Vec<u8>, String> {
    let isa = native_isa(true)?;
    let builder = ObjectBuilder::new(isa, "phoebe_generic", default_libcall_names())
        .map_err(|error| error.to_string())?;
    let mut module = ObjectModule::new(builder);
    emit_into(&mut module, planner, entry, true)?;
    module.finish().emit().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
