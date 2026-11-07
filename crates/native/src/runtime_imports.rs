//! Shared native declarations for the versioned reply runtime ABI.

use super::*;

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
