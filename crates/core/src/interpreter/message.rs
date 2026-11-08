//! Interpreter representation of a concrete message's flattened ABI words.
//! The layout table supplies every width; this module only maps values to and
//! from those parts. A runtime ownership transition happens separately.

use super::*;
use phoebe_layout::Shapes;

pub(super) fn encode(
    data: &[DataDef],
    shapes: &Shapes,
    ty: Type,
    value: &Value,
) -> Result<Vec<u64>, String> {
    let width = shapes.parts(ty)?.len();
    let mut words = vec![0; width];
    encode_into(data, shapes, ty, value, &mut words)?;
    Ok(words)
}

pub(super) fn encode_into(
    data: &[DataDef],
    shapes: &Shapes,
    ty: Type,
    value: &Value,
    words: &mut [u64],
) -> Result<(), String> {
    if words.len() != shapes.parts(ty)?.len() {
        return Err("message shape width mismatch".into());
    }
    match (ty, value) {
        (Type::I64, Value::I64(n)) => words[0] = *n as u64,
        (Type::Bool, Value::Bool(value)) => words[0] = u64::from(*value),
        (Type::Unit, Value::Unit) => {}
        (Type::ReplyToI64, Value::ReplyTo(token)) => {
            words[0] = token.native_word()? as u64;
        }
        (Type::RequestI64, Value::Request(value, token)) => {
            words[0] = *value as u64;
            words[1] = token.native_word()? as u64;
        }
        (Type::ResourceError, Value::ResourceError(error)) => {
            words[0] = error.native_word() as u64;
        }
        (Type::Data(id), Value::Product(value_id, fields)) if id == *value_id => {
            let definition = data.get(id.0).ok_or("unknown message data type")?;
            if definition.fields.len() != fields.len() || definition.fields.is_empty() {
                return Err("message product fields mismatch".into());
            }
            let mut cursor = 0;
            for ((_, field_ty), field) in definition.fields.iter().zip(fields) {
                let width = shapes.parts(*field_ty)?.len();
                encode_into(
                    data,
                    shapes,
                    *field_ty,
                    field,
                    &mut words[cursor..cursor + width],
                )?;
                cursor += width;
            }
        }
        (Type::Data(id), Value::Data(value_id, tag, payload)) if id == *value_id => {
            let definition = data.get(id.0).ok_or("unknown message data type")?;
            let payload_ty = definition
                .payloads
                .get(*tag)
                .ok_or("message variant tag is out of range")?;
            words[0] = *tag as u64;
            match (payload_ty, payload) {
                (Some(payload_ty), Some(payload)) => {
                    let width = shapes.parts(*payload_ty)?.len();
                    encode_into(data, shapes, *payload_ty, payload, &mut words[1..1 + width])?;
                }
                (None, None) => {}
                _ => return Err("message variant payload mismatch".into()),
            }
        }
        _ => return Err("value does not match a transferable message type".into()),
    }
    Ok(())
}

pub(super) fn decode(
    data: &[DataDef],
    shapes: &Shapes,
    ty: Type,
    words: &[u64],
) -> Result<Value, String> {
    if words.len() != shapes.parts(ty)?.len() {
        return Err("message shape width mismatch".into());
    }
    match ty {
        Type::I64 => Ok(Value::I64(words[0] as i64)),
        Type::Bool => match words[0] {
            0 => Ok(Value::Bool(false)),
            1 => Ok(Value::Bool(true)),
            _ => Err("invalid Bool message word".into()),
        },
        Type::Unit => Ok(Value::Unit),
        Type::ReplyToI64 => Ok(Value::ReplyTo(ReplyToken::from_native_word(
            words[0] as i64,
        )?)),
        Type::RequestI64 => Ok(Value::Request(
            words[0] as i64,
            ReplyToken::from_native_word(words[1] as i64)?,
        )),
        Type::ResourceError => Ok(Value::ResourceError(ResourceError::from_native_word(
            words[0] as i64,
        )?)),
        Type::Data(id) => {
            let definition = data.get(id.0).ok_or("unknown message data type")?;
            if !definition.fields.is_empty() {
                let mut fields = Vec::with_capacity(definition.fields.len());
                let mut cursor = 0;
                for (_, field_ty) in &definition.fields {
                    let width = shapes.parts(*field_ty)?.len();
                    fields.push(decode(
                        data,
                        shapes,
                        *field_ty,
                        &words[cursor..cursor + width],
                    )?);
                    cursor += width;
                }
                Ok(Value::Product(id, fields))
            } else {
                let tag = usize::try_from(words[0]).map_err(|_| "invalid message tag")?;
                let payload_ty = definition
                    .payloads
                    .get(tag)
                    .ok_or("message variant tag is out of range")?;
                let payload = match payload_ty {
                    Some(payload_ty) => {
                        let width = shapes.parts(*payload_ty)?.len();
                        Some(Box::new(decode(
                            data,
                            shapes,
                            *payload_ty,
                            &words[1..1 + width],
                        )?))
                    }
                    None => None,
                };
                Ok(Value::Data(id, tag, payload))
            }
        }
        _ => Err("message type is not supported by the interpreter codec".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoebe_runtime::{ReplyTo, TypedReceiveResult, TypedSendResult};
    use std::task::Poll;

    fn data() -> Vec<DataDef> {
        vec![
            DataDef {
                name: "Choice".into(),
                variants: vec!["None".into(), "Right".into()],
                payloads: vec![None, Some(Type::ReplyToI64)],
                fields: vec![],
            },
            DataDef {
                name: "Envelope".into(),
                variants: vec![],
                payloads: vec![],
                fields: vec![
                    ("count".into(), Type::I64),
                    ("choice".into(), Type::Data(DataId(0))),
                    ("flag".into(), Type::Bool),
                ],
            },
        ]
    }

    #[test]
    fn aggregate_round_trip_respects_tag_padding_and_right_offset() {
        let program = Program {
            data: data(),
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        let value = Value::Product(
            DataId(1),
            vec![
                Value::I64(-7),
                Value::Data(DataId(0), 0, None),
                Value::Bool(true),
            ],
        );
        let words = encode(&program.data, &shapes, Type::Data(DataId(1)), &value).unwrap();
        assert_eq!(words, vec![(-7i64) as u64, 0, 0, 1]);
        assert_eq!(
            decode(&program.data, &shapes, Type::Data(DataId(1)), &words).unwrap(),
            value
        );
        let active = Value::Product(
            DataId(1),
            vec![
                Value::I64(-7),
                Value::Data(
                    DataId(0),
                    1,
                    Some(Box::new(Value::ReplyTo(
                        ReplyToken::from_native_word(7).unwrap(),
                    ))),
                ),
                Value::Bool(true),
            ],
        );
        let active_words = encode(&program.data, &shapes, Type::Data(DataId(1)), &active).unwrap();
        assert_eq!(active_words, vec![(-7i64) as u64, 1, 7, 1]);
        assert_eq!(
            decode(&program.data, &shapes, Type::Data(DataId(1)), &active_words).unwrap(),
            active
        );
        assert!(decode(&program.data, &shapes, Type::Data(DataId(1)), &words[..3]).is_err());
    }

    #[test]
    fn invalid_tag_or_boolean_cannot_become_an_interpreter_value() {
        let program = Program {
            data: data(),
            functions: vec![],
        };
        let shapes = Shapes::new(&program).unwrap();
        assert!(decode(&program.data, &shapes, Type::Data(DataId(0)), &[2, 0]).is_err());
        assert!(decode(&program.data, &shapes, Type::Bool, &[2]).is_err());
    }

    struct AggregateRoundTrip;

    impl Task for AggregateRoundTrip {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            let program = Program {
                data: data(),
                functions: vec![],
            };
            let catalog = phoebe_message::MessageCatalog::new(&program)?;
            let ty = Type::Data(DataId(1));
            let descriptor = catalog.descriptor(ty)?;
            let (sender, receiver) = context
                .typed_right_channel(1, descriptor.type_id, descriptor.width, descriptor.rights)
                .map_err(|error| error.to_string())?;
            let (right, reply_receiver) =
                context.reply_typed().map_err(|error| error.to_string())?;
            let value = Value::Product(
                DataId(1),
                vec![
                    Value::I64(11),
                    Value::Data(DataId(0), 1, Some(Box::new(Value::ReplyTo(right.token())))),
                    Value::Bool(true),
                ],
            );
            let words = encode(&program.data, catalog.shapes(), ty, &value)?;
            assert_eq!(
                context.send_typed_words(&sender, descriptor.type_id, &words)?,
                TypedSendResult::Sent
            );
            let mut received = vec![0; descriptor.width];
            assert_eq!(
                context.receive_typed_words(&receiver, descriptor.type_id, &mut received)?,
                TypedReceiveResult::Value
            );
            let decoded = decode(&program.data, catalog.shapes(), ty, &received)?;
            assert_eq!(decoded, value);
            let Value::Product(_, fields) = decoded else {
                unreachable!()
            };
            let Value::Data(_, _, Some(payload)) = &fields[1] else {
                unreachable!()
            };
            let Value::ReplyTo(token) = payload.as_ref() else {
                unreachable!()
            };
            context.close_reply_to(ReplyTo::from_token(*token))?;
            context.cancel_reply(reply_receiver)?;
            context.close_sender(sender)?;
            context.close_receiver(receiver)?;
            Ok(Poll::Ready(11))
        }
    }

    #[test]
    fn aggregate_codec_and_runtime_queue_transfer_the_active_reply_right() {
        let mut scheduler = Scheduler::new(AggregateRoundTrip, 1, 1, 1).unwrap();
        assert_eq!(scheduler.run().unwrap(), 11);
        scheduler.verify().unwrap();
    }
}
