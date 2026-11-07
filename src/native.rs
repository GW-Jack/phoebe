use std::collections::VecDeque;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Write;
use std::mem;
use std::path::Path;
use std::process::{Command, Stdio};

use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    AbiParam, MemFlagsData, Signature, StackSlotData, StackSlotKind, UserFuncName,
    Value as ClValue, types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_native::builder as native_builder;
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::core::{self, CallArg, MatchBinding, Operation, Program, ResultIds, Terminator, Type};
use crate::syntax::ParamMode;

pub struct JitExecutable {
    module: JITModule,
    entry: FuncId,
    arity: usize,
    name: String,
}

pub fn compile_jit(program: &Program, name: &str) -> Result<JitExecutable, String> {
    core::verify(program)?;
    let (_, entry) = program
        .function(name)
        .ok_or_else(|| format!("unknown function `{name}`"))?;
    let arity = entry.blocks[0].params.len();
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
    let ids = compile_functions(&mut module, program, false, None)?;
    module.finalize_definitions().map_err(|e| e.to_string())?;
    let (entry_id, _) = program.function(name).expect("checked above");
    Ok(JitExecutable {
        module,
        entry: ids[entry_id.0].0,
        arity,
        name: name.to_owned(),
    })
}

impl JitExecutable {
    pub fn run(&self, args: &[i64]) -> Result<i64, String> {
        if args.len() != self.arity {
            return Err(format!(
                "`{}` expects {} arguments, got {}",
                self.name,
                self.arity,
                args.len()
            ));
        }
        let pointer = self.module.get_finalized_function(self.entry);
        // The checked I64 signature and owning module make this invocation valid.
        let result = unsafe {
            match args {
                [] => mem::transmute::<*const u8, extern "C" fn() -> i64>(pointer)(),
                [a] => mem::transmute::<*const u8, extern "C" fn(i64) -> i64>(pointer)(*a),
                [a, b] => {
                    mem::transmute::<*const u8, extern "C" fn(i64, i64) -> i64>(pointer)(*a, *b)
                }
                [a, b, c] => mem::transmute::<*const u8, extern "C" fn(i64, i64, i64) -> i64>(
                    pointer,
                )(*a, *b, *c),
                [a, b, c, d] => {
                    mem::transmute::<*const u8, extern "C" fn(i64, i64, i64, i64) -> i64>(pointer)(
                        *a, *b, *c, *d,
                    )
                }
                _ => unreachable!("verifier permits at most four parameters"),
            }
        };
        Ok(result)
    }
}

pub fn run(program: &Program, name: &str, args: &[i64]) -> Result<i64, String> {
    compile_jit(program, name)?.run(args)
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
    compile_functions(&mut module, program, true, None)?;
    module.finish().emit().map_err(|e| e.to_string())
}

/// Print the pre-legalization Cranelift IR that Phoebe submits for each function.
pub fn clif(program: &Program) -> Result<String, String> {
    core::verify(program)?;
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
    let mut output = String::new();
    compile_functions(&mut module, program, false, Some(&mut output))?;
    Ok(output)
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

struct AbiShapes {
    data: Vec<Vec<cranelift_codegen::ir::Type>>,
    pointer_type: cranelift_codegen::ir::Type,
}

impl AbiShapes {
    fn new(program: &Program, pointer_type: cranelift_codegen::ir::Type) -> Self {
        let mut shapes = Self {
            data: Vec::with_capacity(program.data.len()),
            pointer_type,
        };
        for definition in &program.data {
            let shape = if !definition.fields.is_empty() {
                definition
                    .fields
                    .iter()
                    .flat_map(|(_, ty)| shapes.types(*ty))
                    .collect()
            } else {
                let words = definition
                    .payloads
                    .iter()
                    .flatten()
                    .map(|ty| shapes.types(*ty).len())
                    .max()
                    .unwrap_or(0);
                vec![types::I64; 1 + words]
            };
            shapes.data.push(shape);
        }
        shapes
    }

    fn types(&self, ty: Type) -> Vec<cranelift_codegen::ir::Type> {
        match ty {
            Type::I64 => vec![types::I64],
            Type::Bool => vec![types::I8],
            Type::Data(id) => self.data[id.0].clone(),
            Type::RefI64 | Type::RefMutI64 => vec![self.pointer_type],
        }
    }
}

fn compile_functions<M: Module>(
    module: &mut M,
    program: &Program,
    export_main: bool,
    mut clif: Option<&mut String>,
) -> Result<Vec<(FuncId, Signature)>, String> {
    let pointer_type = module.target_config().pointer_type();
    let shapes = AbiShapes::new(program, pointer_type);
    let mut ids = Vec::with_capacity(program.functions.len());
    for (index, function) in program.functions.iter().enumerate() {
        let mut signature = module.make_signature();
        for (index, mode) in function.param_modes.iter().enumerate() {
            let components = if *mode == ParamMode::Own {
                shapes.types(function.value_types[function.blocks[0].params[index].0])
            } else {
                vec![pointer_type]
            };
            signature
                .params
                .extend(components.into_iter().map(AbiParam::new));
        }
        signature.returns.extend(
            shapes
                .types(function.return_type)
                .into_iter()
                .map(AbiParam::new),
        );
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
                    shapes
                        .types(*ty)
                        .into_iter()
                        .map(|component| {
                            let (size, align_shift) = if component == types::I8 {
                                (1, 0)
                            } else {
                                (8, 3)
                            };
                            builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot,
                                size,
                                align_shift,
                            ))
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            builder.append_block_params_for_function_params(blocks[0]);
            for (block_id, block) in function.blocks.iter().enumerate().skip(1) {
                for param in &block.params {
                    for ty in shapes.types(function.value_types[param.0]) {
                        builder.append_block_param(blocks[block_id], ty);
                    }
                }
            }
            let mut values: Vec<Option<Vec<ClValue>>> = vec![None; function.value_types.len()];
            for (block_id, block) in function.blocks.iter().enumerate() {
                let mut offset = 0;
                let raw = builder.block_params(blocks[block_id]);
                for param in &block.params {
                    let count = shapes.types(function.value_types[param.0]).len();
                    values[param.0] = Some(raw[offset..offset + count].to_vec());
                    offset += count;
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
                    Terminator::Match { arms, .. } => arms.iter().map(|arm| arm.target.0).collect(),
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
                    let get = |id: core::ValueId| {
                        values[id.0].clone().expect("verified value is defined")
                    };
                    let one = |id: core::ValueId| get(id)[0];
                    let value = match &instruction.operation {
                        Operation::Const(value) => {
                            Some(vec![builder.ins().iconst(types::I64, *value)])
                        }
                        Operation::Bool(value) => {
                            Some(vec![builder.ins().iconst(types::I8, i64::from(*value))])
                        }
                        Operation::Move(value) | Operation::Copy(value) => Some(get(*value)),
                        Operation::Variant(id, tag, payload) => {
                            let mut components =
                                vec![builder.ins().iconst(types::I64, *tag as i64)];
                            if let Some(value) = payload {
                                let shape = shapes.types(function.value_types[value.0]);
                                for (component, ty) in get(*value).into_iter().zip(shape) {
                                    components.push(if ty == types::I8 {
                                        builder.ins().uextend(types::I64, component)
                                    } else {
                                        component
                                    });
                                }
                            }
                            let words = shapes.data[id.0].len() - 1;
                            while components.len() < 1 + words {
                                components.push(builder.ins().iconst(types::I64, 0));
                            }
                            Some(components)
                        }
                        Operation::Product(_, fields) => {
                            Some(fields.iter().flat_map(|field| get(*field)).collect())
                        }
                        Operation::Field(owner, index) => {
                            let Type::Data(id) = function.value_types[owner.0] else {
                                unreachable!("verified field owner")
                            };
                            let fields = &program.data[id.0].fields;
                            let start = fields[..*index]
                                .iter()
                                .map(|(_, ty)| shapes.types(*ty).len())
                                .sum::<usize>();
                            let width = shapes.types(fields[*index].1).len();
                            Some(get(*owner)[start..start + width].to_vec())
                        }
                        Operation::ReadField(place, index) => {
                            let Type::Data(id) = function.place_types[place.0] else {
                                unreachable!("verified place field owner")
                            };
                            let fields = &program.data[id.0].fields;
                            let offset = fields[..*index]
                                .iter()
                                .map(|(_, ty)| shapes.types(*ty).len())
                                .sum::<usize>();
                            let ty = shapes.types(fields[*index].1)[0];
                            Some(vec![builder.ins().stack_load(
                                pointer_type,
                                ty,
                                places[place.0][offset],
                                0,
                            )])
                        }
                        Operation::Neg(a) => Some(vec![builder.ins().ineg(one(*a))]),
                        Operation::Add(a, b) => Some(vec![builder.ins().iadd(one(*a), one(*b))]),
                        Operation::Sub(a, b) => Some(vec![builder.ins().isub(one(*a), one(*b))]),
                        Operation::Mul(a, b) => Some(vec![builder.ins().imul(one(*a), one(*b))]),
                        Operation::Eq(a, b) => {
                            Some(vec![builder.ins().icmp(IntCC::Equal, one(*a), one(*b))])
                        }
                        Operation::Lt(a, b) => Some(vec![builder.ins().icmp(
                            IntCC::SignedLessThan,
                            one(*a),
                            one(*b),
                        )]),
                        Operation::Call(target, args) => {
                            let callee = module.declare_func_in_func(ids[target.0].0, builder.func);
                            let args = args
                                .iter()
                                .flat_map(|arg| match arg {
                                    CallArg::Value(value) | CallArg::Ref(value, _) => get(*value),
                                    CallArg::Borrow(place, _) => {
                                        vec![builder.ins().stack_addr(
                                            pointer_type,
                                            places[place.0][0],
                                            0,
                                        )]
                                    }
                                })
                                .collect::<Vec<_>>();
                            let call = builder.ins().call(callee, &args);
                            Some(builder.inst_results(call).to_vec())
                        }
                        Operation::LoadRef(reference) => Some(vec![builder.ins().load(
                            types::I64,
                            MemFlagsData::new(),
                            one(*reference),
                            0,
                        )]),
                        Operation::StoreRef(reference, value) => {
                            builder.ins().store(
                                MemFlagsData::new(),
                                one(*value),
                                one(*reference),
                                0,
                            );
                            None
                        }
                        Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
                            for (component, slot) in get(*value).into_iter().zip(&places[place.0]) {
                                builder.ins().stack_store(pointer_type, component, *slot, 0);
                            }
                            None
                        }
                        Operation::Load(place)
                        | Operation::Take(place)
                        | Operation::CopyPlace(place) => Some(
                            shapes
                                .types(function.place_types[place.0])
                                .into_iter()
                                .zip(&places[place.0])
                                .map(|(ty, slot)| {
                                    builder.ins().stack_load(pointer_type, ty, *slot, 0)
                                })
                                .collect(),
                        ),
                    };
                    match (instruction.results, value) {
                        (ResultIds::None, None) => {}
                        (ResultIds::One(result), Some(value)) => values[result.0] = Some(value),
                        _ => unreachable!("verified instruction result arity"),
                    }
                }
                let get =
                    |id: core::ValueId| values[id.0].clone().expect("verified value is defined");
                match &block.terminator {
                    Terminator::Return(value) => {
                        builder.ins().return_(&get(*value));
                    }
                    Terminator::Jump(target, args) => {
                        let args = args
                            .iter()
                            .flat_map(|arg| get(*arg))
                            .map(Into::into)
                            .collect::<Vec<_>>();
                        builder.ins().jump(blocks[target.0], &args);
                    }
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    } => {
                        builder.ins().brif(
                            get(*condition)[0],
                            blocks[then_block.0],
                            &[],
                            blocks[else_block.0],
                            &[],
                        );
                    }
                    Terminator::Match { subject, arms } => {
                        let Type::Data(id) = function.value_types[subject.0] else {
                            unreachable!("verified match subject")
                        };
                        let data = &program.data[id.0];
                        let components = get(*subject);
                        for (tag, arm) in arms.iter().enumerate() {
                            let args = match arm.binding {
                                MatchBinding::Discard => vec![],
                                MatchBinding::Whole => {
                                    components.iter().copied().map(Into::into).collect()
                                }
                                MatchBinding::Payload => {
                                    let ty = data.payloads[tag].expect("verified payload arm");
                                    shapes
                                        .types(ty)
                                        .into_iter()
                                        .enumerate()
                                        .map(|(index, shape)| {
                                            let component = components[1 + index];
                                            if shape == types::I8 {
                                                builder.ins().ireduce(types::I8, component).into()
                                            } else {
                                                component.into()
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                }
                            };
                            if tag + 1 == arms.len() {
                                builder.ins().jump(blocks[arm.target.0], &args);
                            } else {
                                let next = builder.create_block();
                                let expected = builder.ins().iconst(types::I64, tag as i64);
                                let equal =
                                    builder.ins().icmp(IntCC::Equal, components[0], expected);
                                builder
                                    .ins()
                                    .brif(equal, blocks[arm.target.0], &args, next, &[]);
                                builder.switch_to_block(next);
                            }
                        }
                    }
                }
            }
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }
        if let Some(output) = clif.as_deref_mut() {
            writeln!(
                output,
                "; Phoebe function {}\n{}",
                function.name,
                ctx.func.display()
            )
            .map_err(|error| error.to_string())?;
        }
        module
            .define_function(*id, &mut ctx)
            .map_err(|e| e.to_string())?;
        module.clear_context(&mut ctx);
    }
    Ok(ids)
}
