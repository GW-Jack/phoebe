//! Nominal declaration and finite generic data application registration.

use super::*;

fn register_application(
    application: &TypeName,
    span: Span,
    data: &mut Vec<DataDef>,
    data_names: &mut HashMap<String, DataId>,
) -> Result<DataId, Diagnostic> {
    let name = application.key();
    if let Some(id) = data_names.get(&name) {
        return Ok(*id);
    }
    let mut payload = |ty: &TypeName| -> Result<Type, Diagnostic> {
        match ty {
            TypeName::I64 => Ok(Type::I64),
            TypeName::Bool => Ok(Type::Bool),
            TypeName::Unit => Ok(Type::Unit),
            TypeName::SenderI64
            | TypeName::ReceiverI64
            | TypeName::ReplyToI64
            | TypeName::ReplyReceiverI64
            | TypeName::RequestI64
            | TypeName::RequestSenderI64
            | TypeName::RequestReceiverI64 => Err(Diagnostic::new(
                span,
                "generic data cannot carry communication capabilities yet",
            )),
            TypeName::Named(name) if name == "ResourceError" => Ok(Type::ResourceError),
            TypeName::Named(name) => {
                let id = data_names
                    .get(name)
                    .copied()
                    .ok_or_else(|| Diagnostic::new(span, format!("unknown data type {name}")))?;
                if verification::capability_layout::Layout::new(data).leaves(Type::Data(id)) > 0 {
                    return Err(Diagnostic::new(
                        span,
                        "generic data cannot carry communication capabilities yet",
                    ));
                }
                Ok(Type::Data(id))
            }
            TypeName::Option(_) | TypeName::Result(_, _) => {
                register_application(ty, span, data, data_names).map(Type::Data)
            }
        }
    };
    let (variants, payloads) = match application {
        TypeName::Option(value) => (
            vec!["None".into(), "Some".into()],
            vec![None, Some(payload(value)?)],
        ),
        TypeName::Result(ok, err) => (
            vec!["Ok".into(), "Err".into()],
            vec![Some(payload(ok)?), Some(payload(err)?)],
        ),
        _ => unreachable!("only finite generic data applications are registered here"),
    };
    let id = DataId(data.len());
    data_names.insert(name.clone(), id);
    data.push(DataDef {
        name,
        variants,
        payloads,
        fields: vec![],
    });
    Ok(id)
}

pub(super) fn lower_data(
    ast: &syntax::Program,
) -> Result<(Vec<DataDef>, HashMap<String, DataId>), Diagnostic> {
    let mut data = Vec::with_capacity(ast.data.len());
    let mut data_names = HashMap::new();
    for declaration in &ast.data {
        if matches!(
            declaration.name.as_str(),
            "I64"
                | "Bool"
                | "Unit"
                | "Result"
                | "Option"
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
                Some(TypeName::Unit) => Some(Type::Unit),
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
                Some(TypeName::Named(name)) => {
                    let id = data_names.get(name).copied().ok_or_else(|| {
                        Diagnostic::new(
                            variant.span,
                            format!("variant payload type `{name}` must be declared earlier"),
                        )
                    })?;
                    if verification::capability_layout::Layout::new(&data).leaves(Type::Data(id))
                        > 0
                    {
                        return Err(Diagnostic::new(
                            variant.span,
                            "sum variants cannot carry communication capabilities yet",
                        ));
                    }
                    Some(Type::Data(id))
                }
                Some(ty @ (TypeName::Option(_) | TypeName::Result(_, _))) => Some(Type::Data(
                    register_application(ty, variant.span, &mut data, &mut data_names)?,
                )),
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
                    TypeName::Unit => Type::Unit,
                    TypeName::SenderI64 => Type::SenderI64,
                    TypeName::ReceiverI64 => Type::ReceiverI64,
                    TypeName::ReplyToI64 => Type::ReplyToI64,
                    TypeName::ReplyReceiverI64 => Type::ReplyReceiverI64,
                    TypeName::RequestI64 => Type::RequestI64,
                    TypeName::RequestSenderI64 => Type::RequestSenderI64,
                    TypeName::RequestReceiverI64 => Type::RequestReceiverI64,
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
                    ty @ (TypeName::Option(_) | TypeName::Result(_, _)) => Type::Data(
                        register_application(ty, field.span, &mut data, &mut data_names)?,
                    ),
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
    for (application, span) in &ast.applied_types {
        register_application(application, *span, &mut data, &mut data_names)?;
    }
    Ok((data, data_names))
}
