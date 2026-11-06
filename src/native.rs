use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::mem;
use std::path::Path;
use std::process::{Command, Stdio};

use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    AbiParam, MemFlagsData, Signature, StackSlotData, StackSlotKind, UserFuncName, types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_native::builder as native_builder;
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::core::{self, CallArg, Operation, Program, Terminator, Type};
use crate::syntax::ParamMode;

pub fn run(program: &Program, name: &str, args: &[i64]) -> Result<i64, String> {
    core::verify(program)?;
    let (_, entry) = program
        .function(name)
        .ok_or_else(|| format!("unknown function `{name}`"))?;
    if args.len() != entry.blocks[0].params.len() {
        return Err(format!(
            "`{name}` expects {} arguments, got {}",
            entry.blocks[0].params.len(),
            args.len()
        ));
    }
    if entry.param_modes.iter().any(|mode| *mode != ParamMode::Own) {
        return Err("borrowed parameters require a Phoebe caller".into());
    }
    if entry.return_type != Type::I64
        || entry.blocks[0]
            .params
            .iter()
            .any(|param| entry.value_types[param.0] != Type::I64)
    {
        return Err("external native entry requires an I64 signature".into());
    }
    let mut flags = settings::builder();
    flags
        .set("use_colocated_libcalls", "false")
        .map_err(|e| e.to_string())?;
    flags.set("is_pic", "false").map_err(|e| e.to_string())?;
    let isa = native_builder()
        .map_err(|e| format!("unsupported native host: {e}"))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    let mut module = JITModule::new(JITBuilder::with_isa(isa, default_libcall_names()));
    let ids = compile_functions(&mut module, program, false)?;
    module.finalize_definitions().map_err(|e| e.to_string())?;
    let (entry_id, _) = program.function(name).expect("checked above");
    let pointer = module.get_finalized_function(ids[entry_id.0].0);
    // The verifier and arity check establish the I64 signature. The module
    // remains alive while we invoke the host-ABI function pointer.
    let result = unsafe {
        match args {
            [] => mem::transmute::<*const u8, extern "C" fn() -> i64>(pointer)(),
            [a] => mem::transmute::<*const u8, extern "C" fn(i64) -> i64>(pointer)(*a),
            [a, b] => mem::transmute::<*const u8, extern "C" fn(i64, i64) -> i64>(pointer)(*a, *b),
            [a, b, c] => mem::transmute::<*const u8, extern "C" fn(i64, i64, i64) -> i64>(pointer)(
                *a, *b, *c,
            ),
            [a, b, c, d] => mem::transmute::<*const u8, extern "C" fn(i64, i64, i64, i64) -> i64>(
                pointer,
            )(*a, *b, *c, *d),
            _ => return Err("at most four arguments are supported".into()),
        }
    };
    Ok(result)
}

pub fn emit_object(program: &Program) -> Result<Vec<u8>, String> {
    core::verify(program)?;
    let (_, entry) = program
        .function("main")
        .ok_or("standalone executable requires `fn main() -> I64`")?;
    if !entry.param_modes.is_empty() {
        return Err("standalone `main` must take no parameters".into());
    }
    if entry.return_type != Type::I64 {
        return Err("standalone `main` must return I64".into());
    }
    let mut flags = settings::builder();
    flags
        .set("use_colocated_libcalls", "false")
        .map_err(|e| e.to_string())?;
    flags.set("is_pic", "true").map_err(|e| e.to_string())?;
    let isa = native_builder()
        .map_err(|e| format!("unsupported native host: {e}"))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    let builder =
        ObjectBuilder::new(isa, "phoebe", default_libcall_names()).map_err(|e| e.to_string())?;
    let mut module = ObjectModule::new(builder);
    compile_functions(&mut module, program, true)?;
    module.finish().emit().map_err(|e| e.to_string())
}

pub fn build_executable(program: &Program, output: &Path) -> Result<(), String> {
    let object = emit_object(program)?;
    let object_path = output.with_extension(format!("phoebe-{}.o", std::process::id()));
    fs::write(&object_path, object).map_err(|e| format!("{}: {e}", object_path.display()))?;
    let linked = (|| -> Result<(), String> {
        let mut child = Command::new("cc")
            .args(["-x", "c", "-", "-x", "none"])
            .arg(&object_path)
            .arg("-o")
            .arg(output)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to launch C linker: {e}"))?;
        let wrapper = b"#include <stdio.h>\nextern long long phoebe_main(void);\nint main(void) { printf(\"%lld\\n\", phoebe_main()); return 0; }\n";
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(wrapper)
            .map_err(|e| format!("failed to pass entry wrapper to linker: {e}"))?;
        let result = child.wait_with_output().map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(format!(
                "linker failed: {}",
                String::from_utf8_lossy(&result.stderr)
            ));
        }
        Ok(())
    })();
    let _ = fs::remove_file(&object_path);
    linked
}

fn compile_functions<M: Module>(
    module: &mut M,
    program: &Program,
    export_main: bool,
) -> Result<Vec<(FuncId, Signature)>, String> {
    let pointer_type = module.target_config().pointer_type();
    let mut ids = Vec::with_capacity(program.functions.len());
    for (index, function) in program.functions.iter().enumerate() {
        let mut signature = module.make_signature();
        for (index, mode) in function.param_modes.iter().enumerate() {
            let ty = if *mode == ParamMode::Own {
                match function.value_types[function.blocks[0].params[index].0] {
                    Type::I64 | Type::Data(_) => types::I64,
                    Type::Bool => types::I8,
                    Type::RefI64 | Type::RefMutI64 => unreachable!("verified own parameter"),
                }
            } else {
                pointer_type
            };
            signature.params.push(AbiParam::new(ty));
        }
        signature
            .returns
            .push(AbiParam::new(match function.return_type {
                Type::I64 | Type::Data(_) => types::I64,
                Type::Bool => types::I8,
                Type::RefI64 | Type::RefMutI64 => unreachable!("verified return type"),
            }));
        let symbol = if function.name == "main" {
            "phoebe_main".to_owned()
        } else {
            format!("phoebe_fn_{index}")
        };
        let linkage = if export_main && function.name == "main" {
            Linkage::Export
        } else {
            Linkage::Local
        };
        let id = module
            .declare_function(&symbol, linkage, &signature)
            .map_err(|e| e.to_string())?;
        ids.push((id, signature));
    }
    let mut ctx = module.make_context();
    let mut builder_context = FunctionBuilderContext::new();
    for (index, function) in program.functions.iter().enumerate() {
        let (id, signature) = &ids[index];
        ctx.func.signature = signature.clone();
        ctx.func.name = UserFuncName::user(0, id.as_u32());
        {
            let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_context);
            let blocks = function
                .blocks
                .iter()
                .map(|_| builder.create_block())
                .collect::<Vec<_>>();
            let places = function
                .place_types
                .iter()
                .map(|ty| {
                    let (size, align_shift) = match ty {
                        Type::I64 | Type::Data(_) => (8, 3),
                        Type::Bool => (1, 0),
                        Type::RefI64 | Type::RefMutI64 => {
                            unreachable!("references cannot be stored in places")
                        }
                    };
                    builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        size,
                        align_shift,
                    ))
                })
                .collect::<Vec<_>>();
            builder.append_block_params_for_function_params(blocks[0]);
            for (block_id, block) in function.blocks.iter().enumerate().skip(1) {
                for param in &block.params {
                    let ty = match function.value_types[param.0] {
                        Type::I64 | Type::Data(_) => types::I64,
                        Type::Bool => types::I8,
                        Type::RefI64 | Type::RefMutI64 => pointer_type,
                    };
                    builder.append_block_param(blocks[block_id], ty);
                }
            }
            let mut values = vec![None; function.value_types.len()];
            for (block_id, block) in function.blocks.iter().enumerate() {
                for (param, value) in block
                    .params
                    .iter()
                    .zip(builder.block_params(blocks[block_id]))
                {
                    values[param.0] = Some(*value);
                }
            }
            let mut order = Vec::with_capacity(function.blocks.len());
            let mut seen = vec![false; function.blocks.len()];
            let mut ready = VecDeque::from([0usize]);
            seen[0] = true;
            while let Some(block_id) = ready.pop_front() {
                order.push(block_id);
                let successors = match &function.blocks[block_id].terminator {
                    Terminator::Return(_) => vec![],
                    Terminator::Jump(target, _) => vec![target.0],
                    Terminator::Branch {
                        then_block,
                        else_block,
                        ..
                    } => vec![then_block.0, else_block.0],
                };
                for target in successors {
                    if !seen[target] {
                        seen[target] = true;
                        ready.push_back(target);
                    }
                }
            }
            for block_id in order {
                let block = &function.blocks[block_id];
                builder.switch_to_block(blocks[block_id]);
                for instruction in &block.instructions {
                    let get = |id: core::ValueId| values[id.0].expect("verified value is defined");
                    let value = match &instruction.operation {
                        Operation::Const(value) => Some(builder.ins().iconst(types::I64, *value)),
                        Operation::Bool(value) => {
                            Some(builder.ins().iconst(types::I8, i64::from(*value)))
                        }
                        Operation::Variant(_, tag) => {
                            Some(builder.ins().iconst(types::I64, *tag as i64))
                        }
                        Operation::Neg(a) => Some(builder.ins().ineg(get(*a))),
                        Operation::Add(a, b) => Some(builder.ins().iadd(get(*a), get(*b))),
                        Operation::Sub(a, b) => Some(builder.ins().isub(get(*a), get(*b))),
                        Operation::Mul(a, b) => Some(builder.ins().imul(get(*a), get(*b))),
                        Operation::Eq(a, b) => {
                            Some(builder.ins().icmp(IntCC::Equal, get(*a), get(*b)))
                        }
                        Operation::Lt(a, b) => {
                            Some(builder.ins().icmp(IntCC::SignedLessThan, get(*a), get(*b)))
                        }
                        Operation::Call(target, args) => {
                            let callee = module.declare_func_in_func(ids[target.0].0, builder.func);
                            let args = args
                                .iter()
                                .map(|arg| match arg {
                                    CallArg::Value(value) | CallArg::Ref(value, _) => get(*value),
                                    CallArg::Borrow(place, _) => {
                                        builder.ins().stack_addr(pointer_type, places[place.0], 0)
                                    }
                                })
                                .collect::<Vec<_>>();
                            let call = builder.ins().call(callee, &args);
                            Some(builder.inst_results(call)[0])
                        }
                        Operation::LoadRef(reference) => Some(builder.ins().load(
                            types::I64,
                            MemFlagsData::new(),
                            get(*reference),
                            0,
                        )),
                        Operation::StoreRef(reference, value) => {
                            builder.ins().store(
                                MemFlagsData::new(),
                                get(*value),
                                get(*reference),
                                0,
                            );
                            None
                        }
                        Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                            builder.ins().stack_store(
                                pointer_type,
                                get(*value),
                                places[place.0],
                                0,
                            );
                            None
                        }
                        Operation::Load(place) => {
                            let ty = match function.place_types[place.0] {
                                Type::I64 | Type::Data(_) => types::I64,
                                Type::Bool => types::I8,
                                Type::RefI64 | Type::RefMutI64 => {
                                    unreachable!("references cannot be stored in places")
                                }
                            };
                            Some(
                                builder
                                    .ins()
                                    .stack_load(pointer_type, ty, places[place.0], 0),
                            )
                        }
                    };
                    if let Some(result) = instruction.result {
                        values[result.0] = value;
                    }
                }
                let get = |id: core::ValueId| values[id.0].expect("verified value is defined");
                match &block.terminator {
                    Terminator::Return(value) => {
                        builder.ins().return_(&[get(*value)]);
                    }
                    Terminator::Jump(target, args) => {
                        let args = args.iter().map(|arg| get(*arg).into()).collect::<Vec<_>>();
                        builder.ins().jump(blocks[target.0], &args);
                    }
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    } => {
                        builder.ins().brif(
                            get(*condition),
                            blocks[then_block.0],
                            &[],
                            blocks[else_block.0],
                            &[],
                        );
                    }
                }
            }
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }
        module
            .define_function(*id, &mut ctx)
            .map_err(|e| e.to_string())?;
        module.clear_context(&mut ctx);
    }
    Ok(ids)
}
