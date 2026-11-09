//! Shared native declarations for the versioned reply runtime ABI.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct WordsImports {
    pub new: FuncId,
    pub retain: FuncId,
    pub release: FuncId,
    pub set: FuncId,
}

pub(super) fn declare_words_imports<M: Module>(
    module: &mut M,
    pointer: cranelift_codegen::ir::Type,
) -> Result<WordsImports, String> {
    let mut new_signature = module.make_signature();
    new_signature
        .params
        .extend([AbiParam::new(pointer), AbiParam::new(pointer)]);
    new_signature.returns.push(AbiParam::new(pointer));
    let new = module
        .declare_function("phoebe_rt_v1_words_new", Linkage::Import, &new_signature)
        .map_err(|error| error.to_string())?;

    let mut retain_signature = module.make_signature();
    retain_signature.params.push(AbiParam::new(pointer));
    retain_signature.returns.push(AbiParam::new(types::I64));
    let retain = module
        .declare_function(
            "phoebe_rt_v1_words_retain",
            Linkage::Import,
            &retain_signature,
        )
        .map_err(|error| error.to_string())?;

    let mut release_signature = module.make_signature();
    release_signature.params.push(AbiParam::new(pointer));
    let release = module
        .declare_function(
            "phoebe_rt_v1_words_release",
            Linkage::Import,
            &release_signature,
        )
        .map_err(|error| error.to_string())?;

    let mut set_signature = module.make_signature();
    set_signature.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
    ]);
    set_signature.returns.push(AbiParam::new(types::I64));
    let set = module
        .declare_function("phoebe_rt_v1_words_set", Linkage::Import, &set_signature)
        .map_err(|error| error.to_string())?;
    Ok(WordsImports {
        new,
        retain,
        release,
        set,
    })
}

#[derive(Clone, Copy)]
pub(super) struct ReplyImports {
    pub create: FuncId,
    pub close: FuncId,
    pub complete: FuncId,
    pub receive: FuncId,
}

#[derive(Clone, Copy)]
pub(super) struct RequestImports {
    pub create: FuncId,
    pub send: FuncId,
    pub receive: FuncId,
}

#[derive(Clone, Copy)]
pub(super) struct TypedImports {
    pub create: FuncId,
    pub send: FuncId,
    pub receive: FuncId,
}

pub(super) fn declare_typed_imports<M: Module>(
    module: &mut M,
    pointer: cranelift_codegen::ir::Type,
) -> Result<TypedImports, String> {
    let mut create_sig = module.make_signature();
    create_sig.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
        AbiParam::new(pointer),
    ]);
    create_sig.returns.push(AbiParam::new(types::I64));
    let create = module
        .declare_function(
            "phoebe_rt_v1_typed_channel_new",
            Linkage::Import,
            &create_sig,
        )
        .map_err(|error| error.to_string())?;

    let mut traffic_sig = module.make_signature();
    traffic_sig.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
    ]);
    traffic_sig.returns.push(AbiParam::new(types::I64));
    let send = module
        .declare_function(
            "phoebe_rt_v1_send_typed_words",
            Linkage::Import,
            &traffic_sig,
        )
        .map_err(|error| error.to_string())?;
    let receive = module
        .declare_function(
            "phoebe_rt_v1_receive_typed_words",
            Linkage::Import,
            &traffic_sig,
        )
        .map_err(|error| error.to_string())?;
    Ok(TypedImports {
        create,
        send,
        receive,
    })
}

pub(super) fn declare_request_imports<M: Module>(
    module: &mut M,
    pointer: cranelift_codegen::ir::Type,
) -> Result<RequestImports, String> {
    let mut create_sig = module.make_signature();
    create_sig.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
        AbiParam::new(pointer),
    ]);
    create_sig.returns.push(AbiParam::new(types::I64));
    let create = module
        .declare_function(
            "phoebe_rt_v1_request_channel_new_i64",
            Linkage::Import,
            &create_sig,
        )
        .map_err(|error| error.to_string())?;

    let mut send_sig = module.make_signature();
    send_sig.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
    ]);
    send_sig.returns.push(AbiParam::new(types::I64));
    let send = module
        .declare_function("phoebe_rt_v1_send_request_i64", Linkage::Import, &send_sig)
        .map_err(|error| error.to_string())?;

    let mut receive_sig = module.make_signature();
    receive_sig.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
        AbiParam::new(pointer),
    ]);
    receive_sig.returns.push(AbiParam::new(types::I64));
    let receive = module
        .declare_function(
            "phoebe_rt_v1_receive_request_i64",
            Linkage::Import,
            &receive_sig,
        )
        .map_err(|error| error.to_string())?;

    Ok(RequestImports {
        create,
        send,
        receive,
    })
}

pub(super) fn declare_reply_imports<M: Module>(
    module: &mut M,
    pointer: cranelift_codegen::ir::Type,
) -> Result<ReplyImports, String> {
    let mut create_sig = module.make_signature();
    create_sig.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(pointer),
        AbiParam::new(pointer),
    ]);
    create_sig.returns.push(AbiParam::new(types::I64));
    let create = module
        .declare_function("phoebe_rt_v1_reply_new_i64", Linkage::Import, &create_sig)
        .map_err(|error| error.to_string())?;

    let mut action_sig = module.make_signature();
    action_sig.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
    ]);
    action_sig.returns.push(AbiParam::new(types::I64));
    let close = module
        .declare_function("phoebe_rt_v1_reply_close_i64", Linkage::Import, &action_sig)
        .map_err(|error| error.to_string())?;
    let complete = module
        .declare_function(
            "phoebe_rt_v1_reply_complete_i64",
            Linkage::Import,
            &action_sig,
        )
        .map_err(|error| error.to_string())?;

    let mut receive_sig = module.make_signature();
    receive_sig.params.extend([
        AbiParam::new(pointer),
        AbiParam::new(types::I64),
        AbiParam::new(pointer),
    ]);
    receive_sig.returns.push(AbiParam::new(types::I64));
    let receive = module
        .declare_function(
            "phoebe_rt_v1_reply_receive_i64",
            Linkage::Import,
            &receive_sig,
        )
        .map_err(|error| error.to_string())?;
    Ok(ReplyImports {
        create,
        close,
        complete,
        receive,
    })
}
