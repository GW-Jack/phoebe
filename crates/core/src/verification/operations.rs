//! Operation typing and SSA operand enumeration.

use super::*;

pub(super) fn op_type(
    program: &Program,
    function: &Function,
    operation: &Operation,
) -> Result<Option<Type>, String> {
    let ty = |id: ValueId| function.value_types[id.0];
    match operation {
        Operation::ScopeEnter(_) => Ok(None),
        Operation::Const(_) => Ok(Some(Type::I64)),
        Operation::Bool(_) => Ok(Some(Type::Bool)),
        Operation::Move(value) => {
            if !matches!(ty(*value), Type::Data(_)) {
                return Err("move requires a nominal value".into());
            }
            Ok(Some(ty(*value)))
        }
        Operation::Copy(value) => {
            if !matches!(ty(*value), Type::Data(_)) {
                return Err("copy requires a nominal value".into());
            }
            Ok(Some(ty(*value)))
        }
        Operation::Variant(data_id, tag, payload) => {
            let definition = program
                .data
                .get(data_id.0)
                .ok_or_else(|| "invalid data ID".to_owned())?;
            if !definition.fields.is_empty() || *tag >= definition.variants.len() {
                return Err("invalid variant tag".into());
            }
            if definition.payloads[*tag] != payload.map(ty) {
                return Err("variant payload type mismatch".into());
            }
            Ok(Some(Type::Data(*data_id)))
        }
        Operation::Product(data_id, fields) => {
            let definition = program.data.get(data_id.0).ok_or("invalid data ID")?;
            if definition.fields.len() != fields.len() || fields.is_empty() {
                return Err("product field count mismatch".into());
            }
            for (value, (_, expected)) in fields.iter().zip(&definition.fields) {
                if ty(*value) != *expected {
                    return Err("product field type mismatch".into());
                }
            }
            Ok(Some(Type::Data(*data_id)))
        }
        Operation::Field(value, index) => {
            let Type::Data(id) = ty(*value) else {
                return Err("field access requires a product".into());
            };
            program
                .data
                .get(id.0)
                .and_then(|definition| definition.fields.get(*index))
                .map(|(_, ty)| Some(*ty))
                .ok_or_else(|| "invalid field access".into())
        }
        Operation::ReadField(place, index) => {
            let Type::Data(id) = *function
                .place_types
                .get(place.0)
                .ok_or("invalid place ID")?
            else {
                return Err("place field access requires a product".into());
            };
            let field_type = program.data[id.0]
                .fields
                .get(*index)
                .map(|(_, ty)| *ty)
                .ok_or_else(|| "invalid place field access".to_owned())?;
            if matches!(field_type, Type::Data(_)) {
                return Err("nominal place field requires an owning projection".into());
            }
            Ok(Some(field_type))
        }
        Operation::Neg(a) => {
            if ty(*a) != Type::I64 {
                return Err("neg expects I64".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::Not(a) => {
            if ty(*a) != Type::Bool {
                return Err("not expects Bool".into());
            }
            Ok(Some(Type::Bool))
        }
        Operation::Add(a, b) | Operation::Sub(a, b) | Operation::Mul(a, b) => {
            if ty(*a) != Type::I64 || ty(*b) != Type::I64 {
                return Err("arithmetic expects I64".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::Eq(a, b) | Operation::Ne(a, b) => {
            if ty(*a) != ty(*b) || !matches!(ty(*a), Type::I64 | Type::Bool | Type::Data(_)) {
                return Err("equality type mismatch".into());
            }
            if let Type::Data(id) = ty(*a)
                && (!program.data[id.0].fields.is_empty()
                    || program.data[id.0].payloads.iter().any(Option::is_some))
            {
                return Err("product equality and payload sum equality are not implemented".into());
            }
            Ok(Some(Type::Bool))
        }
        Operation::Lt(a, b) | Operation::Le(a, b) | Operation::Gt(a, b) | Operation::Ge(a, b) => {
            if ty(*a) != Type::I64 || ty(*b) != Type::I64 {
                return Err("ordering comparison expects I64".into());
            }
            Ok(Some(Type::Bool))
        }
        Operation::Call(target, args) => {
            let callee = program
                .functions
                .get(target.0)
                .ok_or_else(|| "invalid call target".to_owned())?;
            let callee_params = callee
                .blocks
                .first()
                .ok_or_else(|| "callee has no entry block".to_owned())?
                .params
                .as_slice();
            if args.len() != callee.param_modes.len()
                || args.len() != callee_params.len()
                || check_call_aliases(args).is_err()
            {
                return Err("call signature mismatch".into());
            }
            for ((arg, mode), param) in args.iter().zip(&callee.param_modes).zip(callee_params) {
                let expected = *callee
                    .value_types
                    .get(param.0)
                    .ok_or_else(|| "invalid callee parameter".to_owned())?;
                let valid = match (arg, mode) {
                    (CallArg::Value(value), ParamMode::Own) => ty(*value) == expected,
                    (CallArg::Borrow(place, false), ParamMode::Shared)
                    | (CallArg::Borrow(place, true), ParamMode::Exclusive) => {
                        function.place_types.get(place.0) == Some(&Type::I64)
                    }
                    (CallArg::Ref(value, false), ParamMode::Shared) => {
                        matches!(ty(*value), Type::RefI64 | Type::RefMutI64)
                    }
                    (CallArg::Ref(value, true), ParamMode::Exclusive) => {
                        ty(*value) == Type::RefMutI64
                    }
                    _ => false,
                };
                if !valid {
                    return Err("call signature mismatch".into());
                }
            }
            Ok(Some(callee.return_type))
        }
        Operation::LoadRef(reference) => {
            if !matches!(ty(*reference), Type::RefI64 | Type::RefMutI64) {
                return Err("load requires a reference".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::StoreRef(reference, value) => {
            if ty(*reference) != Type::RefMutI64 || ty(*value) != Type::I64 {
                return Err("store requires an exclusive I64 reference".into());
            }
            Ok(None)
        }
        Operation::CloseHandle(value) => {
            if !matches!(
                ty(*value),
                Type::SenderI64
                    | Type::ReceiverI64
                    | Type::RequestSenderI64
                    | Type::RequestReceiverI64
            ) {
                return Err("close.handle requires a channel endpoint".into());
            }
            Ok(None)
        }
        Operation::CloseReply(value) => {
            if !matches!(ty(*value), Type::ReplyToI64 | Type::ReplyReceiverI64) {
                return Err("close.reply requires a reply capability".into());
            }
            Ok(None)
        }
        Operation::PackRequest { value, right } => {
            if ty(*value) != Type::I64 || ty(*right) != Type::ReplyToI64 {
                return Err("request pack requires I64 and ReplyTo<I64>".into());
            }
            Ok(Some(Type::RequestI64))
        }
        Operation::CloseRequest(value) => {
            if ty(*value) != Type::RequestI64 {
                return Err("close.request requires Request<I64>".into());
            }
            Ok(None)
        }
        Operation::PlaceInit(place, value) | Operation::Store(place, value) => {
            let place_ty = function
                .place_types
                .get(place.0)
                .ok_or_else(|| "invalid place ID".to_owned())?;
            if ty(*value) != *place_ty {
                return Err("place store type mismatch".into());
            }
            Ok(None)
        }
        Operation::Load(place) => match function.place_types.get(place.0) {
            Some(Type::I64) => Ok(Some(Type::I64)),
            Some(Type::Bool) => Ok(Some(Type::Bool)),
            Some(_) => Err("nominal place requires take or field read".into()),
            None => Err("invalid place ID".into()),
        },
        Operation::Take(place) => match function.place_types.get(place.0) {
            Some(ty @ Type::Data(_)) => Ok(Some(*ty)),
            Some(_) => Err("take requires a nominal place".into()),
            None => Err("invalid place ID".into()),
        },
        Operation::CopyPlace(place) => match function.place_types.get(place.0) {
            Some(ty @ Type::Data(_)) => Ok(Some(*ty)),
            Some(_) => Err("copy requires a nominal place".into()),
            None => Err("invalid place ID".into()),
        },
    }
}

/// SSA values read by an operation, used by frame liveness and diagnostics.
pub fn operands(operation: &Operation) -> Vec<ValueId> {
    match operation {
        Operation::Const(_) | Operation::Bool(_) | Operation::ScopeEnter(_) => vec![],
        Operation::Move(value) | Operation::Copy(value) => vec![*value],
        Operation::Variant(_, _, payload) => payload.iter().copied().collect(),
        Operation::Product(_, fields) => fields.clone(),
        Operation::Field(value, _) => vec![*value],
        Operation::PackRequest { value, right } => vec![*value, *right],
        Operation::CloseHandle(a)
        | Operation::CloseReply(a)
        | Operation::CloseRequest(a)
        | Operation::Neg(a)
        | Operation::Not(a)
        | Operation::PlaceInit(_, a)
        | Operation::Store(_, a)
        | Operation::LoadRef(a) => vec![*a],
        Operation::StoreRef(a, b) => vec![*a, *b],
        Operation::Load(_)
        | Operation::Take(_)
        | Operation::CopyPlace(_)
        | Operation::ReadField(_, _) => vec![],
        Operation::Add(a, b)
        | Operation::Sub(a, b)
        | Operation::Mul(a, b)
        | Operation::Eq(a, b)
        | Operation::Ne(a, b)
        | Operation::Lt(a, b)
        | Operation::Le(a, b)
        | Operation::Gt(a, b)
        | Operation::Ge(a, b) => vec![*a, *b],
        Operation::Call(_, args) => args
            .iter()
            .filter_map(|arg| match arg {
                CallArg::Value(value) | CallArg::Ref(value, _) => Some(*value),
                CallArg::Borrow(_, _) => None,
            })
            .collect(),
    }
}
