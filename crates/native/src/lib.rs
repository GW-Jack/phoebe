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

use phoebe_core::{self as core, CallArg, MatchBinding, Operation, Program, Terminator, Type};
use phoebe_frame::{self as frame_layout, FrameWords, FunctionFrame};
use phoebe_runtime::{self as runtime, Context, NativeCallContext, Scheduler, ScopeLimits, Task};
use phoebe_syntax::syntax::ParamMode;

mod poll;
pub use poll::{PollExecutable, PollLeafExecutable, compile_poll_jit, compile_poll_leaf_jit};

pub struct JitExecutable {
    module: JITModule,
    _poll_children: Vec<PollExecutable>,
    entry: FuncId,
    arity: usize,
    name: String,
    uses_runtime: bool,
}

pub fn compile_jit(program: &Program, name: &str) -> Result<JitExecutable, String> {
    core::verify(program)?;
    let (entry_id, entry) = program
        .function(name)
        .ok_or_else(|| format!("unknown function `{name}`"))?;
    if frame_layout::suspendable_functions(program)?[entry_id.0] {
        return Err("suspendable entry requires the retained native backend".into());
    }
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
    let mut poll_children = Vec::new();
    for child in spawned_poll_children(program)? {
        let executable = compile_poll_jit(program, &program.functions[child.0].name)?;
        jit.symbol(
            format!("phoebe_spawn_poll_{}", child.0),
            unsafe { executable.entry() } as *const u8,
        );
        poll_children.push(executable);
    }
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
        _poll_children: poll_children,
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
            call_request: std::ptr::null_mut(),
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
    run_with_limits(program, name, args, ScopeLimits::default())
}

pub fn run_with_limits(
    program: &Program,
    name: &str,
    args: &[i64],
    limits: ScopeLimits,
) -> Result<i64, String> {
    let (id, function) = program
        .function(name)
        .ok_or_else(|| format!("unknown function `{name}`"))?;
    if !frame_layout::suspendable_functions(program)?[id.0] {
        return compile_jit(program, name)?.run_with_limits(args, limits);
    }
    if function.return_type != Type::I64
        || function
            .param_modes
            .iter()
            .any(|mode| *mode != ParamMode::Own)
        || function.blocks[0]
            .params
            .iter()
            .any(|param| function.value_types[param.0] != Type::I64)
    {
        return Err("external native entry requires an owned I64 signature".into());
    }
    if args.len() != function.blocks[0].params.len() {
        return Err(format!(
            "`{name}` expects {} arguments, got {}",
            function.blocks[0].params.len(),
            args.len()
        ));
    }
    let executable = compile_poll_jit(program, name)?;
    // The executable owns the code until the scheduler and all child tasks exit.
    let mut root = unsafe {
        runtime::NativePollTask::new(executable.entry(), executable.layout().total_words)
    }
    .map_err(|error| error.to_string())?;
    for (arg, slot) in args.iter().zip(&executable.layout().entry_params) {
        root.frame_mut()[slot.words.start] = *arg;
    }
    let mut scheduler = Scheduler::new(root, limits.tasks, limits.channels, limits.replies)?;
    scheduler.run()
}

pub fn emit_object(program: &Program) -> Result<Vec<u8>, String> {
    let emitted = emit_object_with_poll_layout(program)?;
    if !emitted.children.is_empty() {
        return Err(
            "mixed direct/poll program emits multiple objects; use build_executable".into(),
        );
    }
    Ok(emitted.root)
}

struct ObjectEmission {
    root: Vec<u8>,
    poll_words: Option<usize>,
    children: Vec<Vec<u8>>,
}

fn spawned_poll_children(program: &Program) -> Result<Vec<core::FunctionId>, String> {
    let suspendable = frame_layout::suspendable_functions(program)?;
    let mut children = vec![false; program.functions.len()];
    for function in &program.functions {
        for block in &function.blocks {
            if let Terminator::Spawn { child, .. } = block.terminator
                && suspendable[child.0]
            {
                children[child.0] = true;
            }
        }
    }
    Ok(children
        .iter()
        .enumerate()
        .filter_map(|(index, found)| found.then_some(core::FunctionId(index)))
        .collect())
}

fn emit_object_with_poll_layout(program: &Program) -> Result<ObjectEmission, String> {
    core::verify(program)?;
    let context_functions = program.runtime_context_functions();
    let (main, entry) = program
        .function("main")
        .ok_or("standalone executable requires `fn main() -> I64`")?;
    if !entry.blocks[0].params.is_empty() {
        return Err("standalone `main` must take no parameters".into());
    }
    if entry.return_type != Type::I64 {
        return Err("standalone `main` must return I64".into());
    }
    if frame_layout::suspendable_functions(program)?[main.0] {
        let (object, words) = poll::emit_poll_object(program, "main", "phoebe_main_poll")?;
        return Ok(ObjectEmission {
            root: object,
            poll_words: Some(words),
            children: Vec::new(),
        });
    }
    let mut poll_objects = Vec::new();
    for child in spawned_poll_children(program)? {
        let (object, _) = poll::emit_poll_object(
            program,
            &program.functions[child.0].name,
            &format!("phoebe_spawn_poll_{}", child.0),
        )?;
        poll_objects.push(object);
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
    Ok(ObjectEmission {
        root: module.finish().emit().map_err(|e| e.to_string())?,
        poll_words: None,
        children: poll_objects,
    })
}

/// Print the pre-legalization Cranelift IR that Phoebe submits for each function.
pub fn clif(program: &Program) -> Result<String, String> {
    core::verify(program)?;
    if let Some((main, _)) = program.function("main")
        && frame_layout::suspendable_functions(program)?[main.0]
    {
        return Ok(compile_poll_jit(program, "main")?.clif().to_owned());
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
    let mut output = String::new();
    let context_functions = program.runtime_context_functions();
    compile_functions(
        &mut module,
        program,
        &context_functions,
        false,
        Some(&mut output),
    )?;
    for child in spawned_poll_children(program)? {
        output.push_str(compile_poll_jit(program, &program.functions[child.0].name)?.clif());
    }
    Ok(output)
}

pub fn build_executable(program: &Program, output: &Path) -> Result<(), String> {
    let emitted = emit_object_with_poll_layout(program)?;
    let runtime_archive = if emitted.poll_words.is_some()
        || !emitted.children.is_empty()
        || program
            .runtime_context_functions()
            .iter()
            .any(|needed| *needed)
    {
        Some(find_runtime_archive()?)
    } else {
        None
    };
    let object_path = output.with_extension(format!("phoebe-{}.o", std::process::id()));
    fs::write(&object_path, emitted.root).map_err(|e| format!("{}: {e}", object_path.display()))?;
    let mut child_paths = Vec::new();
    for (index, child) in emitted.children.into_iter().enumerate() {
        let path = output.with_extension(format!("phoebe-{}-{index}.o", std::process::id()));
        fs::write(&path, child).map_err(|e| format!("{}: {e}", path.display()))?;
        child_paths.push(path);
    }
    let linked = (|| -> Result<(), String> {
        let mut command = Command::new("cc");
        command
            .args(["-x", "c", "-", "-x", "none"])
            .arg(&object_path);
        command.args(&child_paths);
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
        let wrapper = if let Some(words) = emitted.poll_words {
            format!(
                "#include <stdio.h>\nextern long long phoebe_main_poll(void *, long long *);\nextern int phoebe_rt_v1_main_poll(long long (*)(void *, long long *), long long, long long *);\nint main(void) {{ long long value = 0; if (phoebe_rt_v1_main_poll(phoebe_main_poll, {words}, &value)) {{ fputs(\"phoebe runtime failed\\n\", stderr); return 1; }} printf(\"%lld\\n\", value); return 0; }}\n"
            )
        } else if runtime_archive.is_some() {
            "#include <stdio.h>\nextern long long phoebe_main(void *);\nextern int phoebe_rt_v1_main(long long (*)(void *), long long *);\nint main(void) { long long value = 0; if (phoebe_rt_v1_main(phoebe_main, &value)) { fputs(\"phoebe runtime failed\\n\", stderr); return 1; } printf(\"%lld\\n\", value); return 0; }\n".to_owned()
        } else {
            "#include <stdio.h>\nextern long long phoebe_main(void);\nint main(void) { printf(\"%lld\\n\", phoebe_main()); return 0; }\n".to_owned()
        };
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(wrapper.as_bytes())
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
    for path in child_paths {
        let _ = fs::remove_file(path);
    }
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
                .current_dir(
                    option_env!("CARGO_MANIFEST_DIR")
                        .map(|dir| Path::new(dir).join("../.."))
                        .unwrap_or_else(|| PathBuf::from(".")),
                )
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
    // A Buck2-built compiler lives under buck-out rather than target/debug.
    // During local development, build the Cargo runtime archive from the
    // repository checkout when the compiler is invoked there.
    if cfg!(debug_assertions) {
        let root = option_env!("CARGO_MANIFEST_DIR")
            .map(|dir| Path::new(dir).join("../.."))
            .unwrap_or(std::env::current_dir().map_err(|error| error.to_string())?);
        if root.join("crates/runtime/Cargo.toml").is_file() {
            let built = Command::new("cargo")
                .args(["build", "--lib"])
                .current_dir(&root)
                .output()
                .map_err(|error| format!("failed to build runtime archive: {error}"))?;
            if !built.status.success() {
                return Err(format!(
                    "failed to build runtime archive: {}",
                    String::from_utf8_lossy(&built.stderr)
                ));
            }
            let archive = root.join("target/debug/libphoebe.a");
            if archive.is_file() {
                return Ok(archive);
            }
        }
    }
    Err("runtime archive libphoebe.a was not found beside the compiler".into())
}

mod codegen;
pub use codegen::frame_word_layouts;
use codegen::{AbiShapes, compile_functions};
