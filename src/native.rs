use std::mem;

use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::ir::{AbiParam, UserFuncName, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use cranelift_native::builder as native_builder;

use crate::core::{self, Operation, Program};

pub fn run(program: &Program, name: &str, args: &[i64]) -> Result<i64, String> {
    core::verify(program)?;
    let (_, entry) = program
        .function(name)
        .ok_or_else(|| format!("unknown function `{name}`"))?;
    if args.len() != entry.block.params.len() {
        return Err(format!(
            "`{name}` expects {} arguments, got {}",
            entry.block.params.len(),
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
        for _ in &function.block.params {
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
            let block = builder.create_block();
            builder.switch_to_block(block);
            builder.append_block_params_for_function_params(block);
            let mut values = builder.block_params(block).to_vec();
            for instruction in &function.block.instructions {
                let value = match &instruction.operation {
                    Operation::Const(value) => builder.ins().iconst(types::I64, *value),
                    Operation::Neg(a) => builder.ins().ineg(values[a.0]),
                    Operation::Add(a, b) => builder.ins().iadd(values[a.0], values[b.0]),
                    Operation::Sub(a, b) => builder.ins().isub(values[a.0], values[b.0]),
                    Operation::Mul(a, b) => builder.ins().imul(values[a.0], values[b.0]),
                    Operation::Call(target, args) => {
                        let callee = module.declare_func_in_func(ids[target.0].0, builder.func);
                        let args = args.iter().map(|arg| values[arg.0]).collect::<Vec<_>>();
                        let call = builder.ins().call(callee, &args);
                        builder.inst_results(call)[0]
                    }
                };
                values.push(value);
            }
            builder
                .ins()
                .return_(&[values[function.block.return_value.0]]);
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
