use std::collections::VecDeque;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Write;
use std::mem;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::task::Poll;

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

use crate::core::{self, CallArg, MatchBinding, Operation, Program, Terminator, Type};
use crate::frame_layout::{self, FrameWords, FunctionFrame};
use crate::runtime::{self, Context, NativeCallContext, Scheduler, ScopeLimits, Task};
use crate::syntax::ParamMode;

mod poll;
pub use poll::compile_poll_leaf_jit;

pub struct JitExecutable {
    module: JITModule,
    entry: FuncId,
    arity: usize,
    name: String,
    uses_runtime: bool,
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
    let mut jit = JITBuilder::with_isa(isa, default_libcall_names());
    let context_functions = program.runtime_context_functions();
    if context_functions.iter().any(|needed| *needed) {
        jit.symbol(
            "phoebe_rt_v1_channel_new_i64",
            runtime::phoebe_channel_new_i64 as *const u8,
        );
        jit.symbol(
            "phoebe_rt_v1_close_handle_i64",
            runtime::phoebe_close_handle_i64 as *const u8,
        );
        jit.symbol(
            "phoebe_rt_v1_spawn_poll",
            runtime::phoebe_spawn_poll as *const u8,
        );
    }
    let mut module = JITModule::new(jit);
    let ids = compile_functions(&mut module, program, &context_functions, false, None)?;
    module.finalize_definitions().map_err(|e| e.to_string())?;
    let (entry_id, _) = program.function(name).expect("checked above");
    Ok(JitExecutable {
        module,
        entry: ids[entry_id.0].0,
        arity,
        name: name.to_owned(),
        uses_runtime: context_functions[entry_id.0],
    })
}

impl JitExecutable {
    pub fn uses_runtime_context(&self) -> bool {
        self.uses_runtime
    }

    pub fn run(&self, args: &[i64]) -> Result<i64, String> {
        self.run_with_limits(args, ScopeLimits::default())
    }

    pub fn run_with_limits(&self, args: &[i64], limits: ScopeLimits) -> Result<i64, String> {
        if args.len() != self.arity {
            return Err(format!(
                "`{}` expects {} arguments, got {}",
                self.name,
                self.arity,
                args.len()
            ));
        }
        let pointer = self.module.get_finalized_function(self.entry);
        if self.uses_runtime {
            let root = NativeRootTask {
                entry: pointer,
                args: args.to_vec(),
            };
            let mut scheduler =
                Scheduler::new(root, limits.tasks, limits.channels, limits.replies)?;
            return scheduler.run();
        }
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

struct NativeRootTask {
    entry: *const u8,
    args: Vec<i64>,
}

impl Task for NativeRootTask {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let mut fault = false;
        let mut bridge = NativeCallContext {
            context: context as *mut _ as *mut std::ffi::c_void,
            fault: &mut fault,
        };
        let pointer = &mut bridge as *mut NativeCallContext;
        // SAFETY: the JIT module outlives this synchronous poll; signatures
        // include the hidden runtime pointer when uses_runtime is true.
        let result = unsafe {
            match self.args.as_slice() {
                [] => mem::transmute::<*const u8, extern "C" fn(*mut NativeCallContext) -> i64>(
                    self.entry,
                )(pointer),
                [a] => {
                    mem::transmute::<*const u8, extern "C" fn(*mut NativeCallContext, i64) -> i64>(
                        self.entry,
                    )(pointer, *a)
                }
                [a, b] => mem::transmute::<
                    *const u8,
                    extern "C" fn(*mut NativeCallContext, i64, i64) -> i64,
                >(self.entry)(pointer, *a, *b),
                [a, b, c] => mem::transmute::<
                    *const u8,
                    extern "C" fn(*mut NativeCallContext, i64, i64, i64) -> i64,
                >(self.entry)(pointer, *a, *b, *c),
                [a, b, c, d] => mem::transmute::<
                    *const u8,
                    extern "C" fn(*mut NativeCallContext, i64, i64, i64, i64) -> i64,
                >(self.entry)(pointer, *a, *b, *c, *d),
                _ => unreachable!("verifier permits at most four parameters"),
            }
        };
        if fault {
            Err("native runtime ABI fault".into())
        } else {
            Ok(Poll::Ready(result))
        }
    }
}

pub fn run(program: &Program, name: &str, args: &[i64]) -> Result<i64, String> {
    compile_jit(program, name)?.run(args)
}

pub fn emit_object(program: &Program) -> Result<Vec<u8>, String> {
    core::verify(program)?;
    let context_functions = program.runtime_context_functions();
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
    compile_functions(&mut module, program, &context_functions, true, None)?;
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
    let context_functions = program.runtime_context_functions();
    compile_functions(
        &mut module,
        program,
        &context_functions,
        false,
        Some(&mut output),
    )?;
    Ok(output)
}

pub fn build_executable(program: &Program, output: &Path) -> Result<(), String> {
    let object = emit_object(program)?;
    let runtime_archive = if program
        .runtime_context_functions()
        .iter()
        .any(|needed| *needed)
    {
        Some(find_runtime_archive()?)
    } else {
        None
    };
    let object_path = output.with_extension(format!("phoebe-{}.o", std::process::id()));
    fs::write(&object_path, object).map_err(|e| format!("{}: {e}", object_path.display()))?;
    let linked = (|| -> Result<(), String> {
        let mut command = Command::new("cc");
        command
            .args(["-x", "c", "-", "-x", "none"])
            .arg(&object_path);
        if let Some(archive) = &runtime_archive {
            command.arg(archive);
        }
        let mut child = command
            .arg("-o")
            .arg(output)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to launch C linker: {e}"))?;
        let wrapper: &[u8] = if runtime_archive.is_some() {
            b"#include <stdio.h>\nextern long long phoebe_main(void *);\nextern int phoebe_rt_v1_main(long long (*)(void *), long long *);\nint main(void) { long long value = 0; if (phoebe_rt_v1_main(phoebe_main, &value)) { fputs(\"phoebe runtime failed\\n\", stderr); return 1; } printf(\"%lld\\n\", value); return 0; }\n"
        } else {
            b"#include <stdio.h>\nextern long long phoebe_main(void);\nint main(void) { printf(\"%lld\\n\", phoebe_main()); return 0; }\n"
        };
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

fn find_runtime_archive() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("PHOEBE_RUNTIME_LIB") {
        let path = PathBuf::from(path);
        return path
            .is_file()
            .then_some(path)
            .ok_or_else(|| "PHOEBE_RUNTIME_LIB does not name a file".into());
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let Some(directory) = executable.parent() else {
        return Err("cannot locate compiler directory".into());
    };
    let compiler_time = executable
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok();
    let test_profile = if directory.file_name().is_some_and(|name| name == "deps") {
        directory.parent()
    } else {
        None
    };
    for directory in [Some(directory), test_profile].into_iter().flatten() {
        let archive = directory.join("libphoebe.a");
        let archive_time = archive
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok();
        let current = !cfg!(debug_assertions)
            || matches!((archive_time, compiler_time), (Some(archive), Some(compiler)) if archive >= compiler);
        if archive.is_file() && current {
            return Ok(archive);
        }
    }
    if cfg!(debug_assertions) {
        let profile_dir = test_profile.or(Some(directory));
        if let Some(profile_dir) = profile_dir
            && profile_dir.file_name().is_some_and(|name| name == "debug")
            && let Some(target_dir) = profile_dir.parent()
        {
            let built = Command::new("cargo")
                .args(["build", "--lib", "--target-dir"])
                .arg(target_dir)
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .output()
                .map_err(|error| format!("failed to build runtime archive: {error}"))?;
            if !built.status.success() {
                return Err(format!(
                    "failed to build runtime archive: {}",
                    String::from_utf8_lossy(&built.stderr)
                ));
            }
            let archive = profile_dir.join("libphoebe.a");
            if archive.is_file() {
                return Ok(archive);
            }
        }
    }
    Err("runtime archive libphoebe.a was not found beside the compiler".into())
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
            Type::SenderI64 | Type::ReceiverI64 | Type::ResourceError => vec![types::I64],
        }
    }
}

/// Physical word ranges for the retained functions, derived from the same
/// flattened shapes used by direct Cranelift calls and block arguments.
pub fn frame_word_layouts(program: &Program) -> Result<Vec<FrameWords>, String> {
    let frames = frame_layout::analyze(program)?;
    let shapes = AbiShapes::new(program, types::I64);
    frames
        .iter()
        .map(|frame| frame_layout::word_layout(program, frame, |ty| shapes.types(ty).len()))
        .collect()
}

fn compile_functions<M: Module>(
    module: &mut M,
    program: &Program,
    context_functions: &[bool],
    export_main: bool,
    mut clif: Option<&mut String>,
) -> Result<Vec<(FuncId, Signature)>, String> {
    let uses_runtime = context_functions.iter().any(|needed| *needed);
    let pointer_type = module.target_config().pointer_type();
    let shapes = AbiShapes::new(program, pointer_type);
    let runtime_imports = if uses_runtime {
        let mut create_signature = module.make_signature();
        create_signature.params.extend([
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
            AbiParam::new(pointer_type),
            AbiParam::new(pointer_type),
        ]);
        create_signature.returns.push(AbiParam::new(types::I64));
        let create = module
            .declare_function(
                "phoebe_rt_v1_channel_new_i64",
                Linkage::Import,
                &create_signature,
            )
            .map_err(|error| error.to_string())?;
        let mut close_signature = module.make_signature();
        close_signature.params.extend([
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
            AbiParam::new(types::I64),
        ]);
        close_signature.returns.push(AbiParam::new(types::I64));
        let close = module
            .declare_function(
                "phoebe_rt_v1_close_handle_i64",
                Linkage::Import,
                &close_signature,
            )
            .map_err(|error| error.to_string())?;
        let mut spawn_signature = module.make_signature();
        spawn_signature.params.extend([
            AbiParam::new(pointer_type),
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
            AbiParam::new(pointer_type),
            AbiParam::new(types::I64),
        ]);
        spawn_signature.returns.push(AbiParam::new(types::I64));
        let spawn = module
            .declare_function("phoebe_rt_v1_spawn_poll", Linkage::Import, &spawn_signature)
            .map_err(|error| error.to_string())?;
        Some((create, close, spawn))
    } else {
        None
    };
    let mut ids = Vec::with_capacity(program.functions.len());
    for (index, function) in program.functions.iter().enumerate() {
        let mut signature = module.make_signature();
        if context_functions[index] {
            signature.params.push(AbiParam::new(pointer_type));
        }
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
    let mut spawn_wrappers: Vec<Option<(FuncId, FrameWords)>> = vec![None; program.functions.len()];
    for function in &program.functions {
        for block in &function.blocks {
            if let Terminator::Spawn { child, .. } = block.terminator
                && spawn_wrappers[child.0].is_none()
            {
                let empty_frame = FunctionFrame {
                    function: child,
                    points: Vec::new(),
                    value_slots: Vec::new(),
                    place_slots: Vec::new(),
                };
                let layout =
                    frame_layout::word_layout(program, &empty_frame, |ty| shapes.types(ty).len())?;
                let mut signature = module.make_signature();
                signature
                    .params
                    .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
                signature.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function(
                        &format!("phoebe_spawn_poll_{}", child.0),
                        Linkage::Local,
                        &signature,
                    )
                    .map_err(|error| error.to_string())?;
                spawn_wrappers[child.0] = Some((id, layout));
            }
        }
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
            let runtime_context =
                context_functions[index].then(|| builder.block_params(blocks[0])[0]);
            for (block_id, block) in function.blocks.iter().enumerate() {
                let mut offset = usize::from(context_functions[index] && block_id == 0);
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
                    Terminator::ChannelNew {
                        success, failure, ..
                    } => vec![success.0, failure.0],
                    Terminator::ChannelSend { sent, closed, .. } => vec![sent.0, closed.0],
                    Terminator::ChannelReceive { value, end, .. } => vec![value.0, end.0],
                    Terminator::Spawn {
                        success, failure, ..
                    } => vec![success.0, failure.0],
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
                            let mut args = args
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
                            if context_functions[target.0] {
                                let context = runtime_context.expect("caller context propagation");
                                args.insert(0, context);
                            }
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
                        Operation::CloseHandle(value) => {
                            let (_, close, _) = runtime_imports.expect("channel runtime imports");
                            let callee = module.declare_func_in_func(close, builder.func);
                            let kind = match function.value_types[value.0] {
                                Type::SenderI64 => 0,
                                Type::ReceiverI64 => 1,
                                _ => unreachable!("verified handle kind"),
                            };
                            let kind = builder.ins().iconst(types::I64, kind);
                            builder.ins().call(
                                callee,
                                &[runtime_context.expect("runtime context"), one(*value), kind],
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
                    if let Some(result) = instruction.result {
                        values[result.0] = value;
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
                    Terminator::ChannelNew {
                        capacity,
                        success,
                        failure,
                    } => {
                        let (create, _, _) = runtime_imports.expect("channel runtime imports");
                        let callee = module.declare_func_in_func(create, builder.func);
                        let sender_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let receiver_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let sender_address = builder.ins().stack_addr(pointer_type, sender_slot, 0);
                        let receiver_address =
                            builder.ins().stack_addr(pointer_type, receiver_slot, 0);
                        let call = builder.ins().call(
                            callee,
                            &[
                                runtime_context.expect("runtime context"),
                                get(*capacity)[0],
                                sender_address,
                                receiver_address,
                            ],
                        );
                        let status = builder.inst_results(call)[0];
                        let zero = builder.ins().iconst(types::I64, 0);
                        let succeeded = builder.ins().icmp(IntCC::Equal, status, zero);
                        let success_load = builder.create_block();
                        builder.ins().brif(
                            succeeded,
                            success_load,
                            &[],
                            blocks[failure.0],
                            &[status.into()],
                        );
                        builder.switch_to_block(success_load);
                        let sender =
                            builder
                                .ins()
                                .stack_load(pointer_type, types::I64, sender_slot, 0);
                        let receiver =
                            builder
                                .ins()
                                .stack_load(pointer_type, types::I64, receiver_slot, 0);
                        builder
                            .ins()
                            .jump(blocks[success.0], &[sender.into(), receiver.into()]);
                    }
                    Terminator::Spawn {
                        child,
                        args,
                        success,
                        failure,
                    } => {
                        let (_, _, spawn) = runtime_imports.expect("spawn runtime import");
                        let (wrapper, layout) = spawn_wrappers[child.0]
                            .as_ref()
                            .expect("declared spawn wrapper");
                        let wrapper_ref = module.declare_func_in_func(*wrapper, builder.func);
                        let entry = builder.ins().func_addr(pointer_type, wrapper_ref);
                        let mut initial = Vec::new();
                        for (arg, slot) in args.iter().zip(&layout.entry_params) {
                            for (part, (value, ty)) in get(*arg)
                                .into_iter()
                                .zip(shapes.types(function.value_types[arg.0]))
                                .enumerate()
                            {
                                let word = if ty == types::I8 {
                                    builder.ins().uextend(types::I64, value)
                                } else {
                                    value
                                };
                                initial.push((slot.words.start + part, word));
                            }
                        }
                        let initial_ptr = if initial.is_empty() {
                            builder.ins().iconst(pointer_type, 0)
                        } else {
                            let bytes = u32::try_from(
                                initial
                                    .len()
                                    .checked_mul(16)
                                    .ok_or("spawn frame arguments overflow")?,
                            )
                            .map_err(|_| "spawn frame arguments exceed stack slot")?;
                            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot,
                                bytes,
                                3,
                            ));
                            for (index, (word, value)) in initial.iter().enumerate() {
                                let word = builder.ins().iconst(
                                    types::I64,
                                    i64::try_from(*word)
                                        .map_err(|_| "spawn frame offset overflow")?,
                                );
                                builder.ins().stack_store(
                                    pointer_type,
                                    word,
                                    slot,
                                    i32::try_from(index * 16)
                                        .map_err(|_| "spawn argument offset overflow")?,
                                );
                                builder.ins().stack_store(
                                    pointer_type,
                                    *value,
                                    slot,
                                    i32::try_from(index * 16 + 8)
                                        .map_err(|_| "spawn argument offset overflow")?,
                                );
                            }
                            builder.ins().stack_addr(pointer_type, slot, 0)
                        };
                        let transfers = args
                            .iter()
                            .filter_map(|arg| match function.value_types[arg.0] {
                                Type::SenderI64 | Type::ReceiverI64 => Some(get(*arg)[0]),
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        let transfers_ptr = if transfers.is_empty() {
                            builder.ins().iconst(pointer_type, 0)
                        } else {
                            let bytes = u32::try_from(
                                transfers
                                    .len()
                                    .checked_mul(8)
                                    .ok_or("spawn transfer list overflow")?,
                            )
                            .map_err(|_| "spawn transfer list exceeds stack slot")?;
                            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot,
                                bytes,
                                3,
                            ));
                            for (index, value) in transfers.iter().enumerate() {
                                builder.ins().stack_store(
                                    pointer_type,
                                    *value,
                                    slot,
                                    i32::try_from(index * 8)
                                        .map_err(|_| "spawn transfer offset overflow")?,
                                );
                            }
                            builder.ins().stack_addr(pointer_type, slot, 0)
                        };
                        let words = builder.ins().iconst(
                            types::I64,
                            i64::try_from(layout.total_words)
                                .map_err(|_| "spawn frame size overflow")?,
                        );
                        let initial_count = builder.ins().iconst(
                            types::I64,
                            i64::try_from(initial.len())
                                .map_err(|_| "spawn argument count overflow")?,
                        );
                        let transfer_count = builder.ins().iconst(
                            types::I64,
                            i64::try_from(transfers.len())
                                .map_err(|_| "spawn transfer count overflow")?,
                        );
                        let callee = module.declare_func_in_func(spawn, builder.func);
                        let call = builder.ins().call(
                            callee,
                            &[
                                runtime_context.expect("runtime context"),
                                entry,
                                words,
                                initial_ptr,
                                initial_count,
                                transfers_ptr,
                                transfer_count,
                            ],
                        );
                        let status = builder.inst_results(call)[0];
                        let zero = builder.ins().iconst(types::I64, 0);
                        let admitted = builder.ins().icmp(IntCC::Equal, status, zero);
                        builder.ins().brif(
                            admitted,
                            blocks[success.0],
                            &[],
                            blocks[failure.0],
                            &[status.into()],
                        );
                    }
                    Terminator::ChannelSend { .. } | Terminator::ChannelReceive { .. } => {
                        return Err("native channel traffic requires retained task frames".into());
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
    for (child, wrapper) in spawn_wrappers.iter().enumerate() {
        let Some((wrapper_id, layout)) = wrapper else {
            continue;
        };
        let mut signature = module.make_signature();
        signature
            .params
            .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
        signature.returns.push(AbiParam::new(types::I64));
        ctx.func.signature = signature;
        ctx.func.name = UserFuncName::user(0, wrapper_id.as_u32());
        {
            let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_context);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let bridge = builder.block_params(entry)[0];
            let frame = builder.block_params(entry)[1];
            let mut args = Vec::new();
            if context_functions[child] {
                args.push(bridge);
            }
            let function = &program.functions[child];
            for slot in &layout.entry_params {
                for (part, ty) in shapes
                    .types(function.value_types[slot.id.0])
                    .into_iter()
                    .enumerate()
                {
                    let offset = i32::try_from((slot.words.start + part) * 8)
                        .map_err(|_| "spawn frame argument offset overflow")?;
                    args.push(builder.ins().load(ty, MemFlagsData::new(), frame, offset));
                }
            }
            let target = module.declare_func_in_func(ids[child].0, builder.func);
            let call = builder.ins().call(target, &args);
            let result = builder.inst_results(call)[0];
            let offset = i32::try_from(layout.result.start * 8)
                .map_err(|_| "spawn result offset overflow")?;
            builder
                .ins()
                .store(MemFlagsData::new(), result, frame, offset);
            let ready = builder.ins().iconst(types::I64, 0);
            builder.ins().return_(&[ready]);
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }
        if let Some(output) = clif.as_deref_mut() {
            writeln!(
                output,
                "; Phoebe spawn poll wrapper for {}\n{}",
                program.functions[child].name,
                ctx.func.display()
            )
            .map_err(|error| error.to_string())?;
        }
        module
            .define_function(*wrapper_id, &mut ctx)
            .map_err(|e| e.to_string())?;
        module.clear_context(&mut ctx);
    }
    Ok(ids)
}
