use super::*;

mod body;
mod expr;
mod request;
mod spawn_scan;

struct DraftBlock {
    params: Vec<ValueId>,
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

#[derive(Clone)]
struct FunctionSignature {
    id: FunctionId,
    params: Vec<(ParamMode, Type)>,
    return_type: Type,
}

struct CleanupSites {
    returns: Vec<(BlockId, Span)>,
    scopes: Vec<(BlockId, ValueId, Span)>,
    task_returns: Vec<(BlockId, Vec<(TaskScopeId, Span)>)>,
}

struct Lowerer<'a> {
    functions: &'a HashMap<String, FunctionSignature>,
    data: &'a [DataDef],
    data_names: &'a HashMap<String, DataId>,
    spawn_effects: &'a HashMap<String, bool>,
    blocks: Vec<DraftBlock>,
    value_types: Vec<Type>,
    place_types: Vec<Type>,
    param_modes: Vec<ParamMode>,
    return_type: Type,
    current: BlockId,
    implicit_cleanup_returns: Vec<(BlockId, Span)>,
    scoped_cleanup_exits: Vec<(BlockId, ValueId, Span)>,
    task_scope_stack: Vec<(TaskScopeId, Span)>,
    task_scope_returns: Vec<(BlockId, Vec<(TaskScopeId, Span)>)>,
    next_task_scope: usize,
    automatic_checkpoints: bool,
}

impl<'a> Lowerer<'a> {
    fn new(
        functions: &'a HashMap<String, FunctionSignature>,
        data: &'a [DataDef],
        data_names: &'a HashMap<String, DataId>,
        spawn_effects: &'a HashMap<String, bool>,
        params: Vec<(ParamMode, Type)>,
        return_type: Type,
        automatic_checkpoints: bool,
    ) -> Self {
        let mut lowerer = Self {
            functions,
            data,
            data_names,
            spawn_effects,
            blocks: vec![DraftBlock {
                params: Vec::new(),
                instructions: Vec::new(),
                terminator: None,
            }],
            value_types: Vec::new(),
            place_types: Vec::new(),
            param_modes: params.iter().map(|(mode, _)| *mode).collect(),
            return_type,
            current: BlockId(0),
            implicit_cleanup_returns: Vec::new(),
            scoped_cleanup_exits: Vec::new(),
            task_scope_stack: Vec::new(),
            task_scope_returns: Vec::new(),
            next_task_scope: 0,
            automatic_checkpoints,
        };
        for (mode, own_type) in params {
            let ty = match mode {
                ParamMode::Own => own_type,
                ParamMode::Shared => Type::RefI64,
                ParamMode::Exclusive => Type::RefMutI64,
            };
            let id = lowerer.value(ty);
            lowerer.blocks[0].params.push(id);
        }
        lowerer
    }

    fn value(&mut self, ty: Type) -> ValueId {
        let id = ValueId(self.value_types.len());
        self.value_types.push(ty);
        id
    }

    fn block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push(DraftBlock {
            params: Vec::new(),
            instructions: Vec::new(),
            terminator: None,
        });
        id
    }

    fn block_param(&mut self, block: BlockId, ty: Type) -> ValueId {
        let id = self.value(ty);
        self.blocks[block.0].params.push(id);
        id
    }

    fn emit(&mut self, operation: Operation, ty: Type, span: Span) -> ValueId {
        let result = self.value(ty);
        self.blocks[self.current.0].instructions.push(Instruction {
            result: Some(result),
            operation,
            span,
        });
        result
    }

    fn emit_effect(&mut self, operation: Operation, span: Span) {
        self.blocks[self.current.0].instructions.push(Instruction {
            result: None,
            operation,
            span,
        });
    }

    fn place(&mut self, ty: Type) -> PlaceId {
        let id = PlaceId(self.place_types.len());
        self.place_types.push(ty);
        id
    }

    fn terminate(&mut self, terminator: Terminator) {
        assert!(self.blocks[self.current.0].terminator.is_none());
        self.blocks[self.current.0].terminator = Some(terminator);
    }

    fn implicit_return(&mut self, value: ValueId, span: Span) {
        self.terminate(Terminator::Return(value));
        self.implicit_cleanup_returns.push((self.current, span));
        if !self.task_scope_stack.is_empty() {
            self.task_scope_returns
                .push((self.current, self.task_scope_stack.clone()));
        }
    }

    fn require(&self, actual: Type, expected: Type, span: Span) -> Result<(), Diagnostic> {
        if actual == expected {
            Ok(())
        } else {
            Err(Diagnostic::new(
                span,
                format!("expected {expected:?}, got {actual:?}"),
            ))
        }
    }

    fn resource_result_id(&self, span: Span) -> Result<DataId, Diagnostic> {
        let Type::Data(result_id) = self.return_type else {
            return Err(Diagnostic::new(
                span,
                "? requires Result<_, ResourceError> return type",
            ));
        };
        let result = &self.data[result_id.0];
        if !result.name.starts_with("Result<")
            || result.variants.as_slice() != ["Ok", "Err"]
            || result.payloads[1] != Some(Type::ResourceError)
        {
            return Err(Diagnostic::new(
                span,
                "? requires Result<_, ResourceError> return type",
            ));
        }
        Ok(result_id)
    }

    fn finish(self, name: String) -> Result<(Function, CleanupSites), Diagnostic> {
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for block in self.blocks {
            let Some(terminator) = block.terminator else {
                return Err(Diagnostic::new(
                    Span { start: 0, end: 0 },
                    "internal lowering left a block without a terminator",
                ));
            };
            blocks.push(Block {
                params: block.params,
                instructions: block.instructions,
                terminator,
            });
        }
        let mut loan_regions = Vec::new();
        for (block_id, block) in blocks.iter().enumerate() {
            for (instruction_id, instruction) in block.instructions.iter().enumerate() {
                let Operation::Call(_, args) = &instruction.operation else {
                    continue;
                };
                let roots = args
                    .iter()
                    .filter_map(|arg| match arg {
                        CallArg::Borrow(place, exclusive) => {
                            Some(LoanRoot::Place(*place, *exclusive))
                        }
                        CallArg::Ref(value, exclusive) => {
                            Some(LoanRoot::Forwarded(*value, *exclusive))
                        }
                        CallArg::Value(_) => None,
                    })
                    .collect::<Vec<_>>();
                if !roots.is_empty() {
                    loan_regions.push(LoanRegion {
                        id: RegionId(loan_regions.len()),
                        call_block: BlockId(block_id),
                        call_instruction: instruction_id,
                        roots,
                    });
                }
            }
        }
        Ok((
            Function {
                name,
                blocks,
                value_types: self.value_types,
                place_types: self.place_types,
                loan_regions,
                param_modes: self.param_modes,
                return_type: self.return_type,
            },
            CleanupSites {
                returns: self.implicit_cleanup_returns,
                scopes: self.scoped_cleanup_exits,
                task_returns: self.task_scope_returns,
            },
        ))
    }
}

fn register_result_application(
    application: &TypeName,
    span: Span,
    data: &mut Vec<DataDef>,
    data_names: &mut HashMap<String, DataId>,
) -> Result<DataId, Diagnostic> {
    let TypeName::Result(ok, err) = application else {
        unreachable!("only Result applications are registered here")
    };
    let name = application.key();
    if let Some(id) = data_names.get(&name) {
        return Ok(*id);
    }
    let mut payload = |ty: &TypeName| -> Result<Type, Diagnostic> {
        match ty {
            TypeName::I64 => Ok(Type::I64),
            TypeName::Bool => Ok(Type::Bool),
            TypeName::SenderI64
            | TypeName::ReceiverI64
            | TypeName::ReplyToI64
            | TypeName::ReplyReceiverI64
            | TypeName::RequestI64
            | TypeName::RequestSenderI64
            | TypeName::RequestReceiverI64 => Err(Diagnostic::new(
                span,
                "Result cannot carry communication capabilities yet",
            )),
            TypeName::Named(name) if name == "ResourceError" => Ok(Type::ResourceError),
            TypeName::Named(name) => data_names
                .get(name)
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(span, format!("unknown data type {name}"))),
            TypeName::Result(_, _) => {
                register_result_application(ty, span, data, data_names).map(Type::Data)
            }
        }
    };
    let ok = payload(ok)?;
    let err = payload(err)?;
    let id = DataId(data.len());
    data_names.insert(name.clone(), id);
    data.push(DataDef {
        name,
        variants: vec!["Ok".into(), "Err".into()],
        payloads: vec![Some(ok), Some(err)],
        fields: vec![],
    });
    Ok(id)
}

pub fn lower(ast: &syntax::Program) -> Result<Program, Diagnostic> {
    lower_internal(ast, None).map(|(program, _)| program)
}

/// Reuse verified function bodies when their ASTs match under a source offset.
/// The caller supplies reuse hints and span offsets. Core checks those hints,
/// compatible nominal data layouts, and existing function signatures before
/// accepting a cached body. Data and function IDs are remapped after insertions
/// or removals. An unsafe hint falls back to lowering afresh.
pub fn lower_reusing(
    ast: &syntax::Program,
    previous_ast: &syntax::Program,
    previous: &Program,
    reusable: &[bool],
    span_deltas: &[isize],
) -> Result<(Program, usize), Diagnostic> {
    if reusable.len() != ast.functions.len() || span_deltas.len() != ast.functions.len() {
        return Err(Diagnostic::new(
            Span { start: 0, end: 0 },
            "incremental function map length mismatch",
        ));
    }
    lower_internal(ast, Some((previous_ast, previous, reusable, span_deltas)))
}

fn rebase_function(function: &mut Function, delta: isize) -> Result<(), Diagnostic> {
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
fn compatible_data_ids(previous: &[DataDef], current: &[DataDef]) -> Vec<Option<DataId>> {
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

fn same_signature(
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

fn remap_function_data(function: &mut Function, data_ids: &[Option<DataId>]) -> bool {
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

fn remap_function_targets(function: &mut Function, old_to_new: &[Option<FunctionId>]) -> bool {
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

fn lower_internal(
    ast: &syntax::Program,
    reuse: Option<(&syntax::Program, &Program, &[bool], &[isize])>,
) -> Result<(Program, usize), Diagnostic> {
    let spawn_analysis = spawn_scan::analyze(ast);
    let previous_spawn_analysis =
        reuse.map(|(previous_ast, _, _, _)| spawn_scan::analyze(previous_ast));
    if let Some(import) = ast.imports.first() {
        return Err(Diagnostic::new(
            import.span,
            "imports require a phoebe.toml project manifest",
        ));
    }
    let mut data = Vec::with_capacity(ast.data.len());
    let mut data_names = HashMap::new();
    for declaration in &ast.data {
        if matches!(
            declaration.name.as_str(),
            "I64"
                | "Bool"
                | "Result"
                | "ResourceError"
                | "Sender"
                | "Receiver"
                | "Send"
                | "Receive"
                | "ReplyTo"
                | "ReplyReceiver"
                | "Request"
                | "RequestSender"
                | "RequestReceiver"
                | "SendRequest"
                | "ReceiveRequest"
                | "request"
                | "request_channel"
                | "Complete"
                | "Reply"
        ) || data_names.contains_key(&declaration.name)
        {
            return Err(Diagnostic::new(declaration.span, "duplicate data type"));
        }
        let mut variants = Vec::with_capacity(declaration.variants.len());
        let mut payloads = Vec::with_capacity(declaration.variants.len());
        for variant in &declaration.variants {
            if variants.contains(&variant.name) {
                return Err(Diagnostic::new(variant.span, "duplicate variant"));
            }
            let payload = match variant.payload.as_ref().map(|field| &field.ty) {
                None => None,
                Some(TypeName::I64) => Some(Type::I64),
                Some(TypeName::Bool) => Some(Type::Bool),
                Some(
                    TypeName::SenderI64
                    | TypeName::ReceiverI64
                    | TypeName::ReplyToI64
                    | TypeName::ReplyReceiverI64
                    | TypeName::RequestI64
                    | TypeName::RequestSenderI64
                    | TypeName::RequestReceiverI64,
                ) => {
                    return Err(Diagnostic::new(
                        variant.span,
                        "channel endpoints cannot be stored in data yet",
                    ));
                }
                Some(TypeName::Named(name)) if name == "ResourceError" => Some(Type::ResourceError),
                Some(TypeName::Named(name)) => Some(
                    data_names
                        .get(name)
                        .copied()
                        .map(Type::Data)
                        .ok_or_else(|| {
                            Diagnostic::new(
                                variant.span,
                                format!("variant payload type `{name}` must be declared earlier"),
                            )
                        })?,
                ),
                Some(ty @ TypeName::Result(_, _)) => Some(Type::Data(register_result_application(
                    ty,
                    variant.span,
                    &mut data,
                    &mut data_names,
                )?)),
            };
            variants.push(variant.name.clone());
            payloads.push(payload);
        }
        let mut fields = Vec::with_capacity(declaration.fields.len());
        for field in &declaration.fields {
            if fields.iter().any(|(name, _)| name == &field.name) {
                return Err(Diagnostic::new(field.span, "duplicate field"));
            }
            let ty =
                match &field.ty {
                    TypeName::I64 => Type::I64,
                    TypeName::Bool => Type::Bool,
                    TypeName::SenderI64
                    | TypeName::ReceiverI64
                    | TypeName::ReplyToI64
                    | TypeName::ReplyReceiverI64
                    | TypeName::RequestI64
                    | TypeName::RequestSenderI64
                    | TypeName::RequestReceiverI64 => {
                        return Err(Diagnostic::new(
                            field.span,
                            "channel endpoints cannot be stored in data yet",
                        ));
                    }
                    TypeName::Named(name) if name == "ResourceError" => Type::ResourceError,
                    TypeName::Named(name) => data_names
                        .get(name)
                        .copied()
                        .map(Type::Data)
                        .ok_or_else(|| {
                            Diagnostic::new(
                                field.span,
                                format!("product field type `{name}` must be declared earlier"),
                            )
                        })?,
                    ty @ TypeName::Result(_, _) => Type::Data(register_result_application(
                        ty,
                        field.span,
                        &mut data,
                        &mut data_names,
                    )?),
                };
            fields.push((field.name.clone(), ty));
        }
        data_names.insert(declaration.name.clone(), DataId(data.len()));
        data.push(DataDef {
            name: declaration.name.clone(),
            variants,
            payloads,
            fields,
        });
    }
    for (used, name, variants, payloads) in [
        (
            ast.uses_send_outcome,
            "Send<I64>",
            vec!["Sent", "Closed"],
            vec![None, Some(Type::I64)],
        ),
        (
            ast.uses_receive_outcome,
            "Receive<I64>",
            vec!["Value", "End"],
            vec![Some(Type::I64), None],
        ),
        (
            ast.uses_complete_outcome,
            "Complete<I64>",
            vec!["Delivered", "Closed"],
            vec![None, Some(Type::I64)],
        ),
        (
            ast.uses_reply_outcome,
            "Reply<I64>",
            vec!["Value", "End"],
            vec![Some(Type::I64), None],
        ),
    ] {
        if !used {
            continue;
        }
        data_names.insert(name.to_owned(), DataId(data.len()));
        data.push(DataDef {
            name: name.to_owned(),
            variants: variants.into_iter().map(str::to_owned).collect(),
            payloads,
            fields: vec![],
        });
    }
    for (application, span) in &ast.applied_results {
        register_result_application(application, *span, &mut data, &mut data_names)?;
    }
    let resolve = |name: &TypeName, span: Span| -> Result<Type, Diagnostic> {
        match name {
            TypeName::I64 => Ok(Type::I64),
            TypeName::Bool => Ok(Type::Bool),
            TypeName::SenderI64 => Ok(Type::SenderI64),
            TypeName::ReceiverI64 => Ok(Type::ReceiverI64),
            TypeName::ReplyToI64 => Ok(Type::ReplyToI64),
            TypeName::ReplyReceiverI64 => Ok(Type::ReplyReceiverI64),
            TypeName::RequestI64 => Ok(Type::RequestI64),
            TypeName::RequestSenderI64 => Ok(Type::RequestSenderI64),
            TypeName::RequestReceiverI64 => Ok(Type::RequestReceiverI64),
            TypeName::Named(name) if name == "ResourceError" => Ok(Type::ResourceError),
            TypeName::Named(name) => data_names
                .get(name)
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(span, format!("unknown data type {name}"))),
            TypeName::Result(_, _) => data_names
                .get(&name.key())
                .copied()
                .map(Type::Data)
                .ok_or_else(|| Diagnostic::new(span, "unknown Result application")),
        }
    };
    let mut functions = HashMap::new();
    for (index, function) in ast.functions.iter().enumerate() {
        if matches!(function.name.as_str(), "request" | "request_channel") {
            return Err(Diagnostic::new(
                function.name_span,
                "function name is reserved for request operations",
            ));
        }
        if data_names.contains_key(&function.name) {
            return Err(Diagnostic::new(
                function.name_span,
                "function name conflicts with data type",
            ));
        }
        if function.params.len() > 4 {
            return Err(Diagnostic::new(
                function.name_span,
                "at most four parameters in this implementation slice",
            ));
        }
        if functions
            .insert(
                function.name.clone(),
                FunctionSignature {
                    id: FunctionId(index),
                    params: function
                        .params
                        .iter()
                        .map(|param| Ok((param.mode, resolve(&param.ty, param.span)?)))
                        .collect::<Result<Vec<_>, Diagnostic>>()?,
                    return_type: resolve(&function.return_type, function.name_span)?,
                },
            )
            .is_some()
        {
            return Err(Diagnostic::new(
                function.name_span,
                format!("duplicate function {}", function.name),
            ));
        }
    }
    let previous_indices = reuse
        .map(|(previous_ast, _, _, _)| {
            previous_ast
                .functions
                .iter()
                .enumerate()
                .map(|(index, function)| (function.name.as_str(), index))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let data_ids = reuse
        .map(|(_, previous, _, _)| compatible_data_ids(&previous.data, &data))
        .unwrap_or_default();
    let stable_environment = reuse.is_some_and(|(previous_ast, previous, _, _)| {
        previous.functions.len() == previous_ast.functions.len()
            && previous_ast
                .functions
                .iter()
                .zip(&previous.functions)
                .all(|(source, cached)| {
                    source.name == cached.name
                        && functions
                            .get(&source.name)
                            .is_none_or(|signature| same_signature(cached, signature, &data_ids))
                })
    });
    let old_to_new = reuse
        .map(|(_, previous, _, _)| {
            previous
                .functions
                .iter()
                .map(|function| functions.get(&function.name).map(|signature| signature.id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut reused = 0;
    let mut lowered = Vec::with_capacity(ast.functions.len());
    let mut cleanup_sites = Vec::with_capacity(ast.functions.len());
    for (index, function) in ast.functions.iter().enumerate() {
        if stable_environment
            && let Some(&previous_index) = previous_indices.get(function.name.as_str())
            && previous_spawn_analysis.as_ref().is_some_and(|previous| {
                previous.checkpointed[previous_index] == spawn_analysis.checkpointed[index]
            })
            && previous_spawn_analysis.as_ref().is_some_and(|previous| {
                previous.call_effects[previous_index] == spawn_analysis.call_effects[index]
            })
            && let Some((previous_ast, previous, reusable, deltas)) = reuse
            && reusable[index]
            && phoebe_syntax::same_function_with_shift(
                &previous_ast.functions[previous_index],
                function,
                deltas[index],
            )
        {
            let mut cached = previous.functions[previous_index].clone();
            if remap_function_data(&mut cached, &data_ids)
                && remap_function_targets(&mut cached, &old_to_new)
                && rebase_function(&mut cached, deltas[index]).is_ok()
            {
                lowered.push(cached);
                cleanup_sites.push(CleanupSites {
                    returns: Vec::new(),
                    scopes: Vec::new(),
                    task_returns: Vec::new(),
                });
                reused += 1;
                continue;
            }
        }
        let mut lowerer = Lowerer::new(
            &functions,
            &data,
            &data_names,
            &spawn_analysis.effects,
            function
                .params
                .iter()
                .map(|param| Ok((param.mode, resolve(&param.ty, param.span)?)))
                .collect::<Result<Vec<_>, Diagnostic>>()?,
            resolve(&function.return_type, function.name_span)?,
            spawn_analysis.checkpointed[index],
        );
        let mut locals = HashMap::new();
        for (index, param) in function.params.iter().enumerate() {
            let binding = match param.mode {
                ParamMode::Own => Binding::Val(ValueId(index)),
                mode => Binding::Ref(ValueId(index), mode),
            };
            if locals.insert(param.name.clone(), binding).is_some() {
                return Err(Diagnostic::new(
                    param.span,
                    format!("duplicate parameter {}", param.name),
                ));
            }
        }
        let (value, ty) = lowerer
            .body(&function.body, &mut locals)?
            .expect("parser requires a function tail");
        lowerer.require(
            ty,
            resolve(&function.return_type, function.name_span)?,
            function.body.tail.as_ref().expect("tail").span,
        )?;
        lowerer.terminate(Terminator::Return(value));
        let (function, cleanup) = lowerer.finish(function.name.clone())?;
        lowered.push(function);
        cleanup_sites.push(cleanup);
    }
    let mut program = Program {
        data,
        functions: lowered,
    };
    verify_structure(&program)
        .map_err(|message| Diagnostic::new(Span { start: 0, end: 0 }, message))?;
    verify_ownership(&program).map_err(|error| Diagnostic::new(error.span, error.message))?;
    // A single abstract flow pass decides lexical closes in control-flow
    // order. Provisional closes alter the abstract state, so nested aliases
    // close once. The final Core contains only the closes actually needed.
    for ((function, exits), source_function) in program
        .functions
        .iter_mut()
        .zip(&cleanup_sites)
        .zip(&ast.functions)
    {
        if exits.scopes.is_empty() {
            continue;
        }
        let returns = function
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                matches!(block.terminator, Terminator::Return(_)).then_some(BlockId(index))
            })
            .collect::<Vec<_>>();
        let Some((graph, values)) = capability_graph(function, &returns, &exits.scopes) else {
            continue;
        };
        let facts = capability_flow::verify(&graph)
            .map_err(|message| Diagnostic::new(source_function.name_span, message))?;
        let mut spans = vec![None; function.blocks.len()];
        for (block, _, span) in &exits.scopes {
            spans[block.0] = Some(*span);
        }
        for (block, slot) in facts.lexical_closes {
            let span = spans[block].expect("lexical close belongs to a with exit");
            let value = values[slot.0];
            let operation = match function.value_types[value.0] {
                Type::ReplyToI64 | Type::ReplyReceiverI64 => Operation::CloseReply(value),
                Type::RequestI64 => Operation::CloseRequest(value),
                _ => Operation::CloseHandle(value),
            };
            function.blocks[block].instructions.push(Instruction {
                result: None,
                operation,
                span,
            });
        }
    }
    // Treat only source `?` returns as cleanup exits. The exact checker gives
    // each such exit its live slots; then the ordinary verifier checks the
    // completed Core, including every inserted close.
    for ((function, returns), source_function) in program
        .functions
        .iter_mut()
        .zip(&cleanup_sites)
        .zip(&ast.functions)
    {
        let cleanup_blocks = returns
            .returns
            .iter()
            .map(|(block, _)| *block)
            .collect::<Vec<_>>();
        if let Some((graph, values)) = capability_graph(function, &cleanup_blocks, &[]) {
            let facts = capability_flow::verify(&graph)
                .map_err(|message| Diagnostic::new(source_function.name_span, message))?;
            for exit in facts.failures {
                let span = returns
                    .returns
                    .iter()
                    .find(|(block, _)| block.0 == exit.block)
                    .map(|(_, span)| *span)
                    .expect("failure fact belongs to an implicit return");
                for slot in exit.live {
                    let value = values[slot.0];
                    let operation = match function.value_types[value.0] {
                        Type::ReplyToI64 | Type::ReplyReceiverI64 => Operation::CloseReply(value),
                        Type::RequestI64 => Operation::CloseRequest(value),
                        _ => Operation::CloseHandle(value),
                    };
                    function.blocks[exit.block].instructions.push(Instruction {
                        result: None,
                        operation,
                        span,
                    });
                }
            }
        }
    }
    // A fallible operation inside a scope first performs its inferred
    // capability cleanup, then joins the open task regions from inner to
    // outer before returning the original error value.
    for (function, exits) in program.functions.iter_mut().zip(&cleanup_sites) {
        for (block, stack) in &exits.task_returns {
            let Terminator::Return(value) = function.blocks[block.0].terminator else {
                unreachable!("recorded task-scope exit is a return")
            };
            let final_return = BlockId(function.blocks.len());
            function.blocks.push(Block {
                params: Vec::new(),
                instructions: Vec::new(),
                terminator: Terminator::Return(value),
            });
            let mut resume = final_return;
            for (scope, span) in stack.iter().take(stack.len() - 1) {
                let join = BlockId(function.blocks.len());
                function.blocks.push(Block {
                    params: Vec::new(),
                    instructions: Vec::new(),
                    terminator: Terminator::ScopeJoin {
                        scope: *scope,
                        resume,
                        span: *span,
                    },
                });
                resume = join;
            }
            let (scope, span) = stack.last().expect("nonempty task-scope exit");
            function.blocks[block.0].terminator = Terminator::ScopeJoin {
                scope: *scope,
                resume,
                span: *span,
            };
        }
    }
    super::scope_verification::verify(&program)
        .map_err(|message| Diagnostic::new(Span { start: 0, end: 0 }, message))?;
    verify_capability_edges(&program).map_err(|message| {
        let span = ast
            .functions
            .iter()
            .find(|function| message.starts_with(&format!("{}:", function.name)))
            .map(|function| function.name_span)
            .unwrap_or(Span { start: 0, end: 0 });
        Diagnostic::new(span, message)
    })?;
    Ok((program, reused))
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
