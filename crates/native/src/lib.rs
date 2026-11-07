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
    AbiParam, ArgumentPurpose, MemFlagsData, Signature, StackSlotData, StackSlotKind, UserFuncName,
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
mod reachability;
pub use poll::{PollExecutable, PollLeafExecutable, compile_poll_jit, compile_poll_leaf_jit};
use reachability::{spawned_poll_children, spawned_poll_children_in, standalone_direct_functions};

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
            "phoebe_rt_v1_fork_handle_i64",
            runtime::phoebe_fork_handle_i64 as *const u8,
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
    let ids = compile_functions(&mut module, program, &context_functions, false, None, None)?;
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
            let mut scheduler = Scheduler::with_limits(root, limits)?;
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
    let mut scheduler = Scheduler::with_limits(root, limits)?;
    scheduler.run()
}

pub fn emit_object(program: &Program) -> Result<Vec<u8>, String> {
    let (emitted, _) = emit_object_with_poll_layout(program, false, None)?;
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

#[derive(Clone, Default)]
pub struct NativeBuildSession {
    global_key: Option<String>,
    objects: Vec<Option<(String, Vec<u8>)>>,
}

impl NativeBuildSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build with one object per direct function. Returns the number of
    /// objects reused from the preceding successful build in this session.
    pub fn build_executable(&mut self, program: &Program, output: &Path) -> Result<usize, String> {
        let mut next = self.clone();
        let (emitted, reused) = emit_object_with_poll_layout(program, true, Some(&mut next))?;
        link_executable(program, emitted, output)?;
        *self = next;
        Ok(reused)
    }
}

fn emit_object_with_poll_layout(
    program: &Program,
    split_direct: bool,
    mut cache: Option<&mut NativeBuildSession>,
) -> Result<(ObjectEmission, usize), String> {
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
    let suspendable = frame_layout::suspendable_functions(program)?;
    if suspendable[main.0] {
        let (object, words) = poll::emit_poll_object(program, "main", "phoebe_main_poll")?;
        return Ok((
            ObjectEmission {
                root: object,
                poll_words: Some(words),
                children: Vec::new(),
            },
            0,
        ));
    }
    let mut poll_objects = Vec::new();
    let direct = standalone_direct_functions(program, main, &suspendable);
    for child in spawned_poll_children_in(program, &direct)? {
        let (object, _) = poll::emit_poll_object(
            program,
            &program.functions[child.0].name,
            &format!("phoebe_spawn_poll_{}", child.0),
        )?;
        poll_objects.push(object);
    }
    if !split_direct {
        let mut flags = settings::builder();
        flags
            .set("use_colocated_libcalls", "false")
            .map_err(|e| e.to_string())?;
        flags.set("is_pic", "true").map_err(|e| e.to_string())?;
        let isa = native_builder()
            .map_err(|e| format!("unsupported native host: {e}"))?
            .finish(settings::Flags::new(flags))
            .map_err(|e| e.to_string())?;
        let builder = ObjectBuilder::new(isa, "phoebe", default_libcall_names())
            .map_err(|e| e.to_string())?;
        let mut module = ObjectModule::new(builder);
        compile_functions(
            &mut module,
            program,
            &context_functions,
            true,
            None,
            Some(&direct),
        )?;
        return Ok((
            ObjectEmission {
                root: module.finish().emit().map_err(|e| e.to_string())?,
                poll_words: None,
                children: poll_objects,
            },
            0,
        ));
    }
    let global_key = format!(
        "{:?}{:?}{:?}{:?}{:?}",
        program.data,
        program
            .functions
            .iter()
            .map(|function| (
                &function.name,
                &function.param_modes,
                function.return_type,
                function.blocks[0]
                    .params
                    .iter()
                    .map(|id| function.value_types[id.0])
                    .collect::<Vec<_>>(),
            ))
            .collect::<Vec<_>>(),
        context_functions,
        suspendable,
        frame_layout::analyze(program)?
            .into_iter()
            .filter(|frame| suspendable[frame.function.0])
            .collect::<Vec<_>>(),
    );
    let mut direct_objects = Vec::new();
    let mut reused = 0;
    if let Some(session) = cache.as_deref_mut() {
        if session.global_key.as_ref() != Some(&global_key) {
            session.objects.clear();
            session.global_key = Some(global_key);
        }
        session
            .objects
            .resize_with(program.functions.len(), || None);
        session.objects.truncate(program.functions.len());
    }
    for index in 0..program.functions.len() {
        if !direct[index] {
            continue;
        }
        let mut function_key = program.functions[index].clone();
        for block in &mut function_key.blocks {
            for instruction in &mut block.instructions {
                instruction.span.start = 0;
                instruction.span.end = 0;
            }
            if let Terminator::Yield { span, .. } = &mut block.terminator {
                span.start = 0;
                span.end = 0;
            }
        }
        let key = format!("{function_key:?}");
        if let Some((_, object)) = cache
            .as_deref()
            .and_then(|session| session.objects[index].as_ref())
            .filter(|(previous, _)| *previous == key)
        {
            direct_objects.push(object.clone());
            reused += 1;
            continue;
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
        let builder = ObjectBuilder::new(isa, "phoebe", default_libcall_names())
            .map_err(|e| e.to_string())?;
        let mut module = ObjectModule::new(builder);
        let mut selected = vec![false; program.functions.len()];
        selected[index] = true;
        compile_functions(
            &mut module,
            program,
            &context_functions,
            true,
            None,
            Some(&selected),
        )?;
        let object = module.finish().emit().map_err(|e| e.to_string())?;
        if let Some(session) = cache.as_deref_mut() {
            session.objects[index] = Some((key, object.clone()));
        }
        direct_objects.push(object);
    }
    let root = direct_objects
        .drain(..1)
        .next()
        .ok_or("standalone program has no direct entry")?;
    direct_objects.extend(poll_objects);
    Ok((
        ObjectEmission {
            root,
            poll_words: None,
            children: direct_objects,
        },
        reused,
    ))
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
        None,
    )?;
    for child in spawned_poll_children(program)? {
        output.push_str(compile_poll_jit(program, &program.functions[child.0].name)?.clif());
    }
    Ok(output)
}

pub fn build_executable(program: &Program, output: &Path) -> Result<(), String> {
    let (emitted, _) = emit_object_with_poll_layout(program, false, None)?;
    link_executable(program, emitted, output)
}

fn link_executable(
    program: &Program,
    emitted: ObjectEmission,
    output: &Path,
) -> Result<(), String> {
    let runtime_archive = if emitted.poll_words.is_some()
        || program
            .function("main")
            .is_some_and(|(main, _)| program.runtime_context_functions()[main.0])
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
    let test_profile = if directory.file_name().is_some_and(|name| name == "deps") {
        directory.parent()
    } else {
        None
    };
    for directory in [Some(directory), test_profile].into_iter().flatten() {
        let archive = directory.join("libphoebe_runtime.a");
        let cargo_debug = directory.file_name().is_some_and(|name| name == "debug");
        let current = !cfg!(debug_assertions) || !cargo_debug || runtime_archive_current(&archive);
        if archive.is_file() && current {
            return Ok(archive);
        }
    }
    let resource_map = executable.with_file_name(format!(
        "{}.resources.json",
        executable
            .file_name()
            .ok_or("compiler executable has no file name")?
            .to_string_lossy()
    ));
    if let Ok(contents) = fs::read_to_string(&resource_map) {
        // The Buck target declares one resource under this fixed key. Buck
        // writes its location relative to the executable's resource map.
        let prefix = "{\"libphoebe_runtime.a\":\"";
        if let Some(path) = contents
            .strip_prefix(prefix)
            .and_then(|remaining| remaining.strip_suffix("\"}"))
            && !path.contains('"')
            && !path.contains('\\')
            && !Path::new(path).is_absolute()
        {
            let archive = directory.join(path);
            if archive.is_file() {
                return Ok(archive);
            }
        }
    }
    if cfg!(debug_assertions) {
        let profile_dir = test_profile.or(Some(directory));
        if let Some(profile_dir) = profile_dir
            && profile_dir.file_name().is_some_and(|name| name == "debug")
            && let Some(target_dir) = profile_dir.parent()
        {
            let built = Command::new("cargo")
                .args(["build", "-p", "phoebe-runtime", "--lib", "--target-dir"])
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
            let archive = profile_dir.join("libphoebe_runtime.a");
            if archive.is_file() {
                return Ok(archive);
            }
        }
    }
    Err("runtime archive libphoebe_runtime.a was not found beside the compiler".into())
}

fn runtime_archive_current(archive: &Path) -> bool {
    let Ok(archive_time) = archive.metadata().and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let Some(runtime) =
        option_env!("CARGO_MANIFEST_DIR").map(|directory| Path::new(directory).join("../runtime"))
    else {
        return true;
    };
    sources_older_than(&runtime.join("Cargo.toml"), archive_time)
        && sources_older_than(&runtime.join("src"), archive_time)
}

fn sources_older_than(path: &Path, archive_time: std::time::SystemTime) -> bool {
    if path.is_dir() {
        return fs::read_dir(path).is_ok_and(|mut entries| {
            entries.all(|entry| {
                entry.is_ok_and(|entry| sources_older_than(&entry.path(), archive_time))
            })
        });
    }
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified <= archive_time)
}

mod codegen;
pub use codegen::frame_word_layouts;
use codegen::{AbiShapes, compile_functions};
