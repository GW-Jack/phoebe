//! In-process native object reuse and standalone object emission.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::default_libcall_names;
use cranelift_native::builder as native_builder;
use cranelift_object::{ObjectBuilder, ObjectModule};
use phoebe_core::{self as core, Operation, Program, Terminator, Type};
use phoebe_frame as frame_layout;
use sha2::{Digest, Sha256};

use super::{
    ObjectEmission, compile_functions, find_runtime_archive, link_executable, poll,
    spawn_poll_symbol,
};
use crate::reachability::{spawned_poll_children_in, standalone_direct_functions};

#[derive(Clone, Default)]
pub struct NativeBuildSession {
    stable_data_ids: BTreeMap<String, core::DataId>,
    data_layouts: BTreeMap<String, String>,
    facts: BTreeMap<String, String>,
    objects: BTreeMap<String, (String, Vec<u8>)>,
    poll_objects: BTreeMap<String, (String, Vec<u8>, usize)>,
    linked_output: Option<LinkedOutput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

impl FileStamp {
    fn read(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(Self {
            path: path.to_owned(),
            len: metadata.len(),
            modified: metadata.modified().ok()?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
        })
    }
}

#[derive(Clone)]
struct LinkedOutput {
    path: PathBuf,
    input_digest: [u8; 32],
    output_digest: [u8; 32],
    output_stamp: FileStamp,
    output_mode: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeBuildReport {
    pub reused_objects: usize,
    pub linked: bool,
}

fn output_fingerprint(
    path: &Path,
    previous: Option<&LinkedOutput>,
) -> Option<([u8; 32], FileStamp, u32)> {
    let stamp = FileStamp::read(path)?;
    let mode = output_mode(path)?;
    if let Some(previous) = previous
        && previous.output_stamp == stamp
        && previous.output_mode == mode
    {
        return Some((previous.output_digest, stamp, mode));
    }
    let bytes = fs::read(path).ok()?;
    if FileStamp::read(path)? != stamp || output_mode(path)? != mode {
        return None;
    }
    Some((Sha256::digest(bytes).into(), stamp, mode))
}

#[cfg(unix)]
fn output_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(fs::metadata(path).ok()?.permissions().mode())
}

#[cfg(not(unix))]
fn output_mode(path: &Path) -> Option<u32> {
    Some(u32::from(fs::metadata(path).ok()?.permissions().readonly()))
}

fn link_input_digest(program: &Program, emitted: &ObjectEmission) -> Option<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(b"PHOEBE-SESSION-LINK\0\x01");
    hash.update((emitted.root.len() as u64).to_le_bytes());
    hash.update(&emitted.root);
    hash.update((emitted.children.len() as u64).to_le_bytes());
    for child in &emitted.children {
        hash.update((child.len() as u64).to_le_bytes());
        hash.update(child);
    }
    hash.update((emitted.poll_words.unwrap_or(0) as u64).to_le_bytes());
    let uses_runtime = emitted.poll_words.is_some()
        || program
            .function("main")
            .is_some_and(|(main, _)| program.runtime_context_functions()[main.0]);
    hash.update([u8::from(uses_runtime)]);
    if uses_runtime {
        let archive = find_runtime_archive().ok()?;
        hash.update(format!("{:?}", FileStamp::read(&archive)?));
    }
    Some(hash.finalize().into())
}

fn canonical_type(ty: &mut Type, data_ids: &[core::DataId]) {
    if let Type::Data(id) = ty {
        *id = data_ids[id.0];
    }
}

fn canonical_data_layouts(
    program: &Program,
    data_ids: &[core::DataId],
) -> BTreeMap<String, String> {
    program
        .data
        .iter()
        .map(|data| {
            let mut normalized = data.clone();
            for ty in normalized.payloads.iter_mut().flatten() {
                canonical_type(ty, data_ids);
            }
            for (_, ty) in &mut normalized.fields {
                canonical_type(ty, data_ids);
            }
            (data.name.clone(), format!("{normalized:?}"))
        })
        .collect()
}

/// The object relocation names are stable across index shifts. Normalize
/// call and spawn IDs in the body key to those names as well; otherwise an
/// unrelated insertion would still make every caller appear changed.
fn direct_function_key(program: &Program, index: usize, data_ids: &[core::DataId]) -> String {
    let mut function = program.functions[index].clone();
    function.for_each_source_span_mut(|span| *span = phoebe_syntax::Span { start: 0, end: 0 });
    canonical_type(&mut function.return_type, data_ids);
    for ty in function
        .value_types
        .iter_mut()
        .chain(&mut function.place_types)
    {
        canonical_type(ty, data_ids);
    }
    let mut targets = Vec::new();
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            match &mut instruction.operation {
                Operation::Call(target, _) => {
                    targets.push(program.functions[target.0].name.clone());
                    target.0 = 0;
                }
                Operation::Variant(id, ..) | Operation::Product(id, ..) => {
                    *id = data_ids[id.0];
                }
                _ => {}
            }
        }
        if let Terminator::Spawn { child, .. } = &mut block.terminator {
            targets.push(program.functions[child.0].name.clone());
            child.0 = 0;
        }
    }
    format!("{:?}", (function, targets))
}

/// A poll object contains its retained callees, direct helpers, and spawn
/// wrappers. Poll and direct linker names derive from qualified names, so
/// unrelated function-ID shifts do not affect this key. Source spans do not
/// affect code generation. Only layouts reachable from the emitted functions
/// can affect their transport shapes or variant operations.
fn poll_object_key(
    program: &Program,
    root: core::FunctionId,
    symbol: &str,
    data_ids: &[core::DataId],
) -> String {
    let mut reachable = vec![false; program.functions.len()];
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if std::mem::replace(&mut reachable[id.0], true) {
            continue;
        }
        for block in &program.functions[id.0].blocks {
            for instruction in &block.instructions {
                if let Operation::Call(target, _) = instruction.operation {
                    pending.push(target);
                }
            }
            if let Terminator::Spawn { child, .. } = block.terminator {
                pending.push(child);
            }
        }
    }
    let mut used_data = vec![false; program.data.len()];
    let mut pending_data = Vec::new();
    for (index, included) in reachable.iter().copied().enumerate() {
        if !included {
            continue;
        }
        let function = &program.functions[index];
        for ty in function
            .value_types
            .iter()
            .chain(&function.place_types)
            .chain(std::iter::once(&function.return_type))
        {
            if let Type::Data(id) = ty {
                pending_data.push(*id);
            }
        }
    }
    while let Some(id) = pending_data.pop() {
        if std::mem::replace(&mut used_data[id.0], true) {
            continue;
        }
        let data = &program.data[id.0];
        for ty in data
            .payloads
            .iter()
            .flatten()
            .chain(data.fields.iter().map(|(_, ty)| ty))
        {
            if let Type::Data(nested) = ty {
                pending_data.push(*nested);
            }
        }
    }
    let all_layouts = canonical_data_layouts(program, data_ids);
    let layouts = program
        .data
        .iter()
        .zip(used_data)
        .filter(|(_, used)| *used)
        .map(|(data, _)| (data.name.clone(), all_layouts[&data.name].clone()))
        .collect::<BTreeMap<_, _>>();
    let mut key = format!("{:?}", (symbol, layouts));
    for (index, included) in reachable.into_iter().enumerate() {
        if included {
            key.push_str(&format!(
                "\n{}:{}",
                program.functions[index].name,
                direct_function_key(program, index, data_ids)
            ));
        }
    }
    key
}

fn emit_cached_poll_object(
    program: &Program,
    name: &str,
    symbol: &str,
    data_ids: &[core::DataId],
    cache: Option<&mut NativeBuildSession>,
) -> Result<(Vec<u8>, usize, bool), String> {
    let Some(session) = cache else {
        let (object, words) = poll::emit_poll_object(program, name, symbol)?;
        return Ok((object, words, false));
    };
    let (root, _) = program.function(name).ok_or("unknown poll entry")?;
    let key = poll_object_key(program, root, symbol, data_ids);
    if let Some((_, object, words)) = session
        .poll_objects
        .get(symbol)
        .filter(|(previous, _, _)| *previous == key)
    {
        return Ok((object.clone(), *words, true));
    }
    let (object, words) = poll::emit_poll_object(program, name, symbol)?;
    session
        .poll_objects
        .insert(symbol.into(), (key, object.clone(), words));
    Ok((object, words, false))
}

impl NativeBuildSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build with one object per direct function and retained poll entry.
    /// Returns the number of objects reused from this session's last build.
    pub fn build_executable(&mut self, program: &Program, output: &Path) -> Result<usize, String> {
        Ok(self
            .build_executable_report(program, output)?
            .reused_objects)
    }

    /// An identical link input and intact output can reuse the executable.
    /// Core verification and object-key checks still run on every build.
    pub fn build_executable_report(
        &mut self,
        program: &Program,
        output: &Path,
    ) -> Result<NativeBuildReport, String> {
        let mut next = self.clone();
        let (emitted, reused) = emit_object_with_poll_layout(program, true, Some(&mut next))?;
        let input_digest = link_input_digest(program, &emitted);
        if let (Some(input_digest), Some(previous)) = (input_digest, &self.linked_output)
            && previous.path == output
            && previous.input_digest == input_digest
            && output_fingerprint(output, Some(previous)).is_some_and(|(digest, _, mode)| {
                digest == previous.output_digest && mode == previous.output_mode
            })
        {
            *self = next;
            return Ok(NativeBuildReport {
                reused_objects: reused,
                linked: false,
            });
        }
        link_executable(program, &emitted, output)?;
        let after_link = link_input_digest(program, &emitted);
        next.linked_output = input_digest
            .filter(|before| Some(*before) == after_link)
            .zip(output_fingerprint(output, None))
            .map(
                |(input_digest, (output_digest, output_stamp, output_mode))| LinkedOutput {
                    path: output.to_owned(),
                    input_digest,
                    output_digest,
                    output_stamp,
                    output_mode,
                },
            );
        *self = next;
        Ok(NativeBuildReport {
            reused_objects: reused,
            linked: true,
        })
    }
}

pub(super) fn emit_object_with_poll_layout(
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
    let data_ids = if let Some(session) = cache.as_deref_mut() {
        program
            .data
            .iter()
            .map(|data| {
                let next = core::DataId(session.stable_data_ids.len());
                *session
                    .stable_data_ids
                    .entry(data.name.clone())
                    .or_insert(next)
            })
            .collect::<Vec<_>>()
    } else {
        (0..program.data.len()).map(core::DataId).collect()
    };
    let suspendable = frame_layout::suspendable_functions(program)?;
    if suspendable[main.0] {
        if let Some(session) = cache.as_deref_mut() {
            session
                .poll_objects
                .retain(|symbol, _| symbol == "phoebe_main_poll");
        }
        let (object, words, reused) = emit_cached_poll_object(
            program,
            "main",
            "phoebe_main_poll",
            &data_ids,
            cache.as_deref_mut(),
        )?;
        return Ok((
            ObjectEmission {
                root: object,
                poll_words: Some(words),
                children: Vec::new(),
            },
            usize::from(reused),
        ));
    }
    let mut poll_objects = Vec::new();
    let direct = standalone_direct_functions(program, main, &suspendable);
    let children = spawned_poll_children_in(program, &direct)?;
    if let Some(session) = cache.as_deref_mut() {
        let symbols = children
            .iter()
            .map(|child| spawn_poll_symbol(&program.functions[child.0].name))
            .collect::<BTreeSet<_>>();
        session
            .poll_objects
            .retain(|symbol, _| symbols.contains(symbol));
    }
    let mut reused_poll = 0;
    for child in children {
        let name = &program.functions[child.0].name;
        let (object, _, reused) = emit_cached_poll_object(
            program,
            name,
            &spawn_poll_symbol(name),
            &data_ids,
            cache.as_deref_mut(),
        )?;
        reused_poll += usize::from(reused);
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
    let data_layouts = canonical_data_layouts(program, &data_ids);
    let mut frame_facts = vec![None; program.functions.len()];
    for frame in frame_layout::analyze(program)? {
        let function = &program.functions[frame.function.0];
        frame_facts[frame.function.0] = Some(format!(
            "{:?}",
            (
                frame.points,
                frame.value_slots,
                frame.place_slots,
                function
                    .value_types
                    .iter()
                    .map(|ty| {
                        let mut ty = *ty;
                        canonical_type(&mut ty, &data_ids);
                        ty
                    })
                    .collect::<Vec<_>>(),
                function
                    .place_types
                    .iter()
                    .map(|ty| {
                        let mut ty = *ty;
                        canonical_type(&mut ty, &data_ids);
                        ty
                    })
                    .collect::<Vec<_>>(),
            )
        ));
    }
    let facts = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            (
                function.name.clone(),
                format!(
                    "{:?}",
                    (
                        &function.param_modes,
                        {
                            let mut ty = function.return_type;
                            canonical_type(&mut ty, &data_ids);
                            ty
                        },
                        function.blocks[0]
                            .params
                            .iter()
                            .map(|id| {
                                let mut ty = function.value_types[id.0];
                                canonical_type(&mut ty, &data_ids);
                                ty
                            })
                            .collect::<Vec<_>>(),
                        context_functions[index],
                        suspendable[index],
                        &frame_facts[index],
                    )
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut direct_objects = Vec::new();
    let mut reused = reused_poll;
    if let Some(session) = cache.as_deref_mut() {
        if session
            .data_layouts
            .iter()
            .any(|(name, old)| data_layouts.get(name) != Some(old))
            || session
                .facts
                .iter()
                .any(|(name, old)| facts.get(name).is_some_and(|now| now != old))
        {
            session.objects.clear();
        }
        session.data_layouts = data_layouts;
        session.facts = facts;
        session
            .objects
            .retain(|name, _| session.facts.contains_key(name));
    }
    for index in 0..program.functions.len() {
        if !direct[index] {
            continue;
        }
        let key = direct_function_key(program, index, &data_ids);
        if let Some((_, object)) = cache
            .as_deref()
            .and_then(|session| session.objects.get(&program.functions[index].name))
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
            session
                .objects
                .insert(program.functions[index].name.clone(), (key, object.clone()));
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

#[cfg(test)]
mod link_cache_tests {
    use super::*;

    #[test]
    fn same_size_archive_replacement_changes_file_stamp() {
        let path =
            std::env::temp_dir().join(format!("phoebe-archive-stamp-{}", std::process::id()));
        let replacement = path.with_extension("replacement");
        fs::write(&path, b"first").unwrap();
        let before = FileStamp::read(&path).unwrap();
        fs::write(&replacement, b"other").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert_ne!(FileStamp::read(&path), Some(before));
        fs::remove_file(path).unwrap();
    }
}
