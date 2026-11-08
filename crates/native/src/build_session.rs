//! In-process native object reuse and standalone object emission.

use std::collections::BTreeMap;
use std::path::Path;

use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::default_libcall_names;
use cranelift_native::builder as native_builder;
use cranelift_object::{ObjectBuilder, ObjectModule};
use phoebe_core::{self as core, Operation, Program, Terminator, Type};
use phoebe_frame as frame_layout;

use super::{ObjectEmission, compile_functions, link_executable, poll, spawn_poll_symbol};
use crate::reachability::{spawned_poll_children_in, standalone_direct_functions};

#[derive(Clone, Default)]
pub struct NativeBuildSession {
    stable_data_ids: BTreeMap<String, core::DataId>,
    data_layouts: BTreeMap<String, String>,
    facts: BTreeMap<String, String>,
    objects: BTreeMap<String, (String, Vec<u8>)>,
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

impl NativeBuildSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build with one object per direct function. Returns the number of
    /// objects reused from the preceding successful build in this session.
    pub fn build_executable(&mut self, program: &Program, output: &Path) -> Result<usize, String> {
        let mut next = self.clone();
        let (emitted, reused) = emit_object_with_poll_layout(program, true, Some(&mut next))?;
        link_executable(program, &emitted, output)?;
        *self = next;
        Ok(reused)
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
            &spawn_poll_symbol(&program.functions[child.0].name),
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
    let mut reused = 0;
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
