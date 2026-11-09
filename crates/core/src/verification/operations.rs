//! Operation typing and SSA operand enumeration.

use super::*;

pub(super) fn op_type(
    program: &Program,
    function: &Function,
    operation: &Operation,
    layout: &capability_layout::Layout,
    generics: Option<&CheckedModule>,
) -> Result<Option<Type>, String> {
    let ty = |id: ValueId| function.value_types[id.0];
    match operation {
        Operation::ScopeEnter(_) => Ok(None),
        Operation::Const(_) => Ok(Some(Type::I64)),
        Operation::Bool(_) => Ok(Some(Type::Bool)),
        Operation::Unit => Ok(Some(Type::Unit)),
        Operation::ArrayI64(elements) => {
            if elements.len() > 32 || elements.iter().any(|value| ty(*value) != Type::I64) {
                return Err("inline I64 array expects at most 32 I64 elements".into());
            }
            Ok(Some(Type::ArrayI64(elements.len())))
        }
        Operation::ArrayGet(array, index) => {
            let Type::ArrayI64(len) = ty(*array) else {
                return Err("array.get requires an I64 array".into());
            };
            if *index >= len {
                return Err("constant array index is out of bounds".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::ArrayLen(array) => {
            if !matches!(ty(*array), Type::ArrayI64(_)) {
                return Err("array.len requires an I64 array".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::ArrayGetPlace(place, index) => {
            let Some(Type::ArrayI64(len)) = function.place_types.get(place.0) else {
                return Err("place array.get requires an I64 array".into());
            };
            if *index >= *len {
                return Err("constant array index is out of bounds".into());
            }
            Ok(Some(Type::I64))
        }
        Operation::ArraySetPlace(place, index, value) => {
            let Some(Type::ArrayI64(len)) = function.place_types.get(place.0) else {
                return Err("place array.set requires an I64 array".into());
            };
            if *index >= *len {
                return Err("constant array index is out of bounds".into());
            }
            if ty(*value) != Type::I64 {
                return Err("array.set expects I64".into());
            }
            Ok(None)
        }
        Operation::Move(value) => {
            if !matches!(ty(*value), Type::Data(_) | Type::ArrayI64(_)) {
                return Err("move requires a nominal value".into());
            }
            Ok(Some(ty(*value)))
        }
        Operation::Copy(value) | Operation::InferredCopy(value) => {
            if !matches!(ty(*value), Type::Data(_) | Type::ArrayI64(_)) {
                return Err("copy requires a nominal value".into());
            }
            if layout.leaves(ty(*value)) > 0 {
                return Err("copy cannot duplicate communication capabilities".into());
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
        Operation::DestBegin(place) | Operation::DestPublish(place) => {
            let Some(Type::Data(id)) = function.place_types.get(place.0) else {
                return Err("destination requires a product place".into());
            };
            let fields = &program.data[id.0].fields;
            if fields.len() != 2
                || !fields
                    .iter()
                    .all(|(_, ty)| matches!(ty, Type::I64 | Type::Bool | Type::Unit))
            {
                return Err("destination requires a two-field closed scalar product".into());
            }
            if matches!(operation, Operation::DestPublish(_)) {
                Ok(Some(Type::Data(*id)))
            } else {
                Ok(None)
            }
        }
        Operation::DestField(place, index, value) => {
            let Some(Type::Data(id)) = function.place_types.get(place.0) else {
                return Err("destination field requires a product place".into());
            };
            let expected = program.data[id.0]
                .fields
                .get(*index)
                .map(|(_, ty)| *ty)
                .ok_or("invalid destination field")?;
            if ty(*value) != expected {
                return Err("destination field type mismatch".into());
            }
            Ok(None)
        }
        Operation::Field(value, index) => {
            let Type::Data(id) = ty(*value) else {
                return Err("field access requires a product".into());
            };
            let field = program
                .data
                .get(id.0)
                .and_then(|definition| definition.fields.get(*index))
                .map(|(_, ty)| *ty)
                .ok_or_else(|| "invalid field access".to_owned())?;
            if layout.leaves(field) > 0 {
                return Err("capability field requires whole-product unpack".into());
            }
            Ok(Some(field))
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
            if layout.leaves(field_type) > 0 {
                return Err("capability place field requires whole-product unpack".into());
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
                        function.place_types.get(place.0)
                            == Some(&match expected {
                                Type::RefBool | Type::RefMutBool => Type::Bool,
                                _ => Type::I64,
                            })
                    }
                    (CallArg::Ref(value, false), ParamMode::Shared) => {
                        matches!(
                            (ty(*value), expected),
                            (Type::RefI64 | Type::RefMutI64, Type::RefI64)
                                | (Type::RefBool | Type::RefMutBool, Type::RefBool)
                        )
                    }
                    (CallArg::Ref(value, true), ParamMode::Exclusive) => ty(*value) == expected,
                    _ => false,
                };
                if !valid {
                    return Err("call signature mismatch".into());
                }
            }
            Ok(Some(callee.return_type))
        }
        Operation::GenericCall {
            callee,
            type_args,
            args,
        } => {
            let module = generics.ok_or("unlinked generic call")?;
            if type_args
                .iter()
                .any(|ty| matches!(ty, Type::Data(id) if id.0 >= program.data.len()))
            {
                return Err("invalid generic type argument".into());
            }
            let actual = args.iter().map(|value| ty(*value)).collect::<Vec<_>>();
            let resolved = crate::generic_calls::instantiate(module, *callee, type_args, &actual)?;
            Ok(Some(resolved.result))
        }
        Operation::BorrowBegin(place, exclusive) => {
            let referent = function
                .place_types
                .get(place.0)
                .copied()
                .ok_or("invalid place ID")?;
            Type::parameter(
                if *exclusive {
                    ParamMode::Exclusive
                } else {
                    ParamMode::Shared
                },
                referent,
            )
            .map(Some)
            .ok_or_else(|| "borrow requires an I64 or Bool place".into())
        }
        Operation::BorrowReborrow(parent, exclusive) => {
            let parent_type = ty(*parent);
            let referent = parent_type
                .referent()
                .ok_or("reborrow requires a reference")?;
            if *exclusive && !matches!(parent_type, Type::RefMutI64 | Type::RefMutBool) {
                return Err("exclusive reborrow requires an exclusive parent".into());
            }
            Ok(Type::parameter(
                if *exclusive {
                    ParamMode::Exclusive
                } else {
                    ParamMode::Shared
                },
                referent,
            ))
        }
        Operation::BorrowEnd(reference) => {
            if ty(*reference).referent().is_none() {
                return Err("borrow end requires a reference".into());
            }
            Ok(None)
        }
        Operation::LoadRef(reference) => match ty(*reference) {
            Type::RefI64 | Type::RefMutI64 => Ok(Some(Type::I64)),
            Type::RefBool | Type::RefMutBool => Ok(Some(Type::Bool)),
            _ => Err("load requires a reference".into()),
        },
        Operation::StoreRef(reference, value) => {
            if !matches!(
                (ty(*reference), ty(*value)),
                (Type::RefMutI64, Type::I64) | (Type::RefMutBool, Type::Bool)
            ) {
                return Err("store requires a matching exclusive reference".into());
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
                    | Type::TypedSender(_)
                    | Type::TypedReceiver(_)
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
        Operation::CloseCaps(value) => {
            if !matches!(ty(*value), Type::Data(_)) {
                return Err("close.caps requires a nominal value".into());
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
            Some(Type::Unit) => Ok(Some(Type::Unit)),
            Some(_) => Err("nominal place requires take or field read".into()),
            None => Err("invalid place ID".into()),
        },
        Operation::Take(place) => match function.place_types.get(place.0) {
            Some(ty)
                if matches!(ty, Type::Data(_) | Type::ArrayI64(_))
                    || capability_layout::Layout::bare_right(*ty) =>
            {
                Ok(Some(*ty))
            }
            Some(_) => Err("take requires an owned place".into()),
            None => Err("invalid place ID".into()),
        },
        Operation::CopyPlace(place) | Operation::InferredCopyPlace(place) => match function
            .place_types
            .get(place.0)
        {
            Some(ty @ (Type::Data(_) | Type::ArrayI64(_))) if layout.leaves(*ty) == 0 => {
                Ok(Some(*ty))
            }
            Some(Type::Data(_)) => Err("copy cannot duplicate communication capabilities".into()),
            Some(_) => Err("copy requires a nominal place".into()),
            None => Err("invalid place ID".into()),
        },
    }
}

/// SSA values read by an operation, used by frame liveness and diagnostics.
pub fn operands(operation: &Operation) -> Vec<ValueId> {
    match operation {
        Operation::Const(_) | Operation::Bool(_) | Operation::Unit | Operation::ScopeEnter(_) => {
            vec![]
        }
        Operation::Move(value) | Operation::Copy(value) | Operation::InferredCopy(value) => {
            vec![*value]
        }
        Operation::Variant(_, _, payload) => payload.iter().copied().collect(),
        Operation::Product(_, fields) | Operation::ArrayI64(fields) => fields.clone(),
        Operation::Field(value, _) | Operation::ArrayGet(value, _) | Operation::ArrayLen(value) => {
            vec![*value]
        }
        Operation::PackRequest { value, right } => vec![*value, *right],
        Operation::CloseHandle(a)
        | Operation::CloseReply(a)
        | Operation::CloseRequest(a)
        | Operation::CloseCaps(a)
        | Operation::Neg(a)
        | Operation::Not(a)
        | Operation::PlaceInit(_, a)
        | Operation::Store(_, a)
        | Operation::ArraySetPlace(_, _, a)
        | Operation::DestField(_, _, a)
        | Operation::BorrowEnd(a)
        | Operation::BorrowReborrow(a, _)
        | Operation::LoadRef(a) => vec![*a],
        Operation::StoreRef(a, b) => vec![*a, *b],
        Operation::Load(_)
        | Operation::Take(_)
        | Operation::CopyPlace(_)
        | Operation::InferredCopyPlace(_)
        | Operation::ReadField(_, _)
        | Operation::ArrayGetPlace(_, _)
        | Operation::DestBegin(_)
        | Operation::DestPublish(_)
        | Operation::BorrowBegin(_, _) => vec![],
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
        Operation::GenericCall { args, .. } => args.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_product_exposes_closed_sibling_but_not_right_field() {
        let program = Program {
            data: vec![DataDef {
                name: "Envelope".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![("sender".into(), Type::SenderI64), ("n".into(), Type::I64)],
            }],
            functions: vec![],
        };
        let function = Function {
            name: "f".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![Type::Data(DataId(0))],
            place_types: vec![],
            loan_regions: vec![],
            blocks: vec![],
        };
        let layout = capability_layout::Layout::new(&program.data);
        assert_eq!(
            op_type(
                &program,
                &function,
                &Operation::Copy(ValueId(0)),
                &layout,
                None
            ),
            Err("copy cannot duplicate communication capabilities".into())
        );
        assert_eq!(
            op_type(
                &program,
                &function,
                &Operation::Field(ValueId(0), 0),
                &layout,
                None
            ),
            Err("capability field requires whole-product unpack".into())
        );
        assert_eq!(
            op_type(
                &program,
                &function,
                &Operation::Field(ValueId(0), 1),
                &layout,
                None
            ),
            Ok(Some(Type::I64))
        );
    }
}
