//! Safe checked-Core reuse after source and nominal ID shifts.

use super::*;

pub(super) fn rebase_function(function: &mut Function, delta: isize) -> Result<(), Diagnostic> {
    let mut error = None;
    function.for_each_source_span_mut(|span| {
        let Some(start) = span.start.checked_add_signed(delta) else {
            error.get_or_insert_with(|| Diagnostic::new(*span, "incremental source span overflow"));
            return;
        };
        let Some(end) = span.end.checked_add_signed(delta) else {
            error.get_or_insert_with(|| Diagnostic::new(*span, "incremental source span overflow"));
            return;
        };
        *span = Span { start, end };
    });
    error.map_or(Ok(()), Err)
}

fn remap_type(ty: Type, data_ids: &[Option<DataId>]) -> Option<Type> {
    match ty {
        Type::Data(id) => data_ids.get(id.0).copied().flatten().map(Type::Data),
        _ => Some(ty),
    }
}

// Data definitions are acyclic: a field or payload can only name a type
// declared earlier. A layout remains compatible only if its whole reachable
// representation is compatible, even when the immediate field list is equal.
pub(super) fn compatible_data_ids(
    previous: &[DataDef],
    current: &[DataDef],
) -> Vec<Option<DataId>> {
    let names = current
        .iter()
        .enumerate()
        .map(|(index, def)| (def.name.as_str(), DataId(index)))
        .collect::<HashMap<_, _>>();
    let mut mapped = vec![None; previous.len()];
    for (old_index, old) in previous.iter().enumerate() {
        let Some(&new_id) = names.get(old.name.as_str()) else {
            continue;
        };
        let new = &current[new_id.0];
        if old.variants == new.variants
            && old.payloads.len() == new.payloads.len()
            && old
                .payloads
                .iter()
                .zip(&new.payloads)
                .all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some(a), Some(b)) => remap_type(*a, &mapped) == Some(*b),
                    _ => false,
                })
            && old.fields.len() == new.fields.len()
            && old
                .fields
                .iter()
                .zip(&new.fields)
                .all(|((a_name, a), (b_name, b))| {
                    a_name == b_name && remap_type(*a, &mapped) == Some(*b)
                })
        {
            mapped[old_index] = Some(new_id);
        }
    }
    mapped
}

pub(super) fn same_signature(
    cached: &Function,
    signature: &FunctionSignature,
    data_ids: &[Option<DataId>],
) -> bool {
    remap_type(cached.return_type, data_ids) == Some(signature.return_type)
        && cached.param_modes
            == signature
                .params
                .iter()
                .map(|(mode, _)| *mode)
                .collect::<Vec<_>>()
        && cached.blocks.first().is_some_and(|entry| {
            entry.params.len() == signature.params.len()
                && entry
                    .params
                    .iter()
                    .zip(&signature.params)
                    .all(|(value, (mode, ty))| {
                        cached
                            .value_types
                            .get(value.0)
                            .copied()
                            .and_then(|ty| remap_type(ty, data_ids))
                            == Some(match mode {
                                ParamMode::Own => *ty,
                                ParamMode::Shared => Type::RefI64,
                                ParamMode::Exclusive => Type::RefMutI64,
                            })
                    })
        })
}

pub(super) fn remap_function_data(function: &mut Function, data_ids: &[Option<DataId>]) -> bool {
    let Some(return_type) = remap_type(function.return_type, data_ids) else {
        return false;
    };
    function.return_type = return_type;
    for ty in function
        .value_types
        .iter_mut()
        .chain(&mut function.place_types)
    {
        let Some(mapped) = remap_type(*ty, data_ids) else {
            return false;
        };
        *ty = mapped;
    }
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            let id = match &mut instruction.operation {
                Operation::Variant(id, ..) | Operation::Product(id, ..) => id,
                _ => continue,
            };
            let Some(mapped) = data_ids.get(id.0).copied().flatten() else {
                return false;
            };
            *id = mapped;
        }
    }
    true
}

pub(super) fn remap_function_targets(
    function: &mut Function,
    old_to_new: &[Option<FunctionId>],
) -> bool {
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            if let Operation::Call(target, _) = &mut instruction.operation {
                let Some(mapped) = old_to_new.get(target.0).copied().flatten() else {
                    return false;
                };
                *target = mapped;
            }
        }
        if let Terminator::Spawn { child, .. } = &mut block.terminator {
            let Some(mapped) = old_to_new.get(child.0).copied().flatten() else {
                return false;
            };
            *child = mapped;
        }
    }
    true
}

#[cfg(test)]
mod data_reuse_tests {
    use super::*;

    #[test]
    fn nested_layout_change_invalidates_containing_type() {
        let old = vec![
            DataDef {
                name: "Inner".into(),
                variants: vec!["A".into()],
                payloads: vec![Some(Type::I64)],
                fields: vec![],
            },
            DataDef {
                name: "Outer".into(),
                variants: vec!["B".into()],
                payloads: vec![Some(Type::Data(DataId(0)))],
                fields: vec![],
            },
        ];
        let mut new = old.clone();
        new.insert(
            0,
            DataDef {
                name: "Extra".into(),
                variants: vec!["A".into()],
                payloads: vec![None],
                fields: vec![],
            },
        );
        new[2].payloads[0] = Some(Type::Data(DataId(1)));
        assert_eq!(
            compatible_data_ids(&old, &new),
            vec![Some(DataId(1)), Some(DataId(2))]
        );
        new[1].payloads[0] = Some(Type::Bool);
        assert_eq!(compatible_data_ids(&old, &new), vec![None, None]);
    }
}
