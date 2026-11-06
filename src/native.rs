use std::collections::VecDeque;
use std::mem;

use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{AbiParam, UserFuncName, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use cranelift_native::builder as native_builder;

use crate::core::{self, Operation, Program, Terminator, Type};

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
    let mut ids = Vec::with_capacity(program.functions.len());
    for function in &program.functions {
        let mut signature = module.make_signature();
        for _ in &function.blocks[0].params {
            signature.params.push(AbiParam::new(types::I64));
        }
        signature.returns.push(AbiParam::new(types::I64));
        let id = module
            .declare_function(&function.name, Linkage::Local, &signature)
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
            builder.append_block_params_for_function_params(blocks[0]);
            for (block_id, block) in function.blocks.iter().enumerate().skip(1) {
                for param in &block.params {
                    let ty = match function.value_types[param.0] {
                        Type::I64 => types::I64,
                        Type::Bool => types::I8,
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
                        Operation::Const(value) => builder.ins().iconst(types::I64, *value),
                        Operation::Bool(value) => {
                            builder.ins().iconst(types::I8, i64::from(*value))
                        }
                        Operation::Neg(a) => builder.ins().ineg(get(*a)),
                        Operation::Add(a, b) => builder.ins().iadd(get(*a), get(*b)),
                        Operation::Sub(a, b) => builder.ins().isub(get(*a), get(*b)),
                        Operation::Mul(a, b) => builder.ins().imul(get(*a), get(*b)),
                        Operation::Eq(a, b) => builder.ins().icmp(IntCC::Equal, get(*a), get(*b)),
                        Operation::Lt(a, b) => {
                            builder.ins().icmp(IntCC::SignedLessThan, get(*a), get(*b))
                        }
                        Operation::Call(target, args) => {
                            let callee = module.declare_func_in_func(ids[target.0].0, builder.func);
                            let args = args.iter().map(|arg| get(*arg)).collect::<Vec<_>>();
                            let call = builder.ins().call(callee, &args);
                            builder.inst_results(call)[0]
                        }
                    };
                    values[instruction.result.0] = Some(value);
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
