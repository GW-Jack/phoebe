//! Versioned native ABI for concrete, fixed-width typed channel messages.
//! The plan pointer contains the stable word encoding in `typed_queue::wire`.

use super::*;
use crate::typed_queue::{NATIVE_PLAN_MAX_WORDS, PlanDecodeError};

fn fault(bridge: *mut NativeCallContext) -> i64 {
    if !bridge.is_null() {
        // SAFETY: generated callers supply a live bridge and fault flag.
        unsafe { *(*bridge).fault = true };
    }
    255
}

/// Status 0 writes both handles. Resource failure writes neither. The plan is
/// decoded once and owned by the preallocated queue for its lifetime.
///
/// # Safety
///
/// `bridge` must borrow a live task context exclusively. `plan_words` must
/// point to `plan_len` readable u64 words, and the distinct outputs must each
/// point to one aligned writable i64. All pointers remain live for this call.
#[unsafe(export_name = "phoebe_rt_v1_typed_channel_new")]
pub unsafe extern "C" fn phoebe_typed_channel_new(
    bridge: *mut NativeCallContext,
    capacity: i64,
    type_id: u64,
    width: i64,
    plan_words: *const u64,
    plan_len: i64,
    sender_out: *mut i64,
    receiver_out: *mut i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<i64, ()> {
        if bridge.is_null()
            || sender_out.is_null()
            || receiver_out.is_null()
            || sender_out == receiver_out
        {
            return Err(());
        }
        let capacity = match usize::try_from(capacity) {
            Ok(capacity) => capacity,
            Err(_) => return Ok(native::resource_code(ResourceError::InvalidChannelCapacity)),
        };
        let width = usize::try_from(width).map_err(|_| ())?;
        let plan_len = usize::try_from(plan_len).map_err(|_| ())?;
        if !(2..=NATIVE_PLAN_MAX_WORDS).contains(&plan_len) || plan_words.is_null() {
            return Err(());
        }
        let words = unsafe { std::slice::from_raw_parts(plan_words, plan_len) };
        let plan = match TypedRightPlan::decode_native_words(words) {
            Ok(plan) => plan,
            Err(PlanDecodeError::Allocation) => {
                return Ok(native::resource_code(ResourceError::Allocation));
            }
            Err(PlanDecodeError::Invalid) => return Err(()),
        };
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        Ok(
            match context.typed_right_channel(capacity, type_id, width, plan) {
                Ok((sender, receiver)) => {
                    unsafe {
                        *sender_out = sender.token().native_word().map_err(|_| ())?;
                        *receiver_out = receiver.token().native_word().map_err(|_| ())?;
                    }
                    0
                }
                Err(error) => native::resource_code(error),
            },
        )
    }));
    match outcome {
        Ok(Ok(code)) => code,
        _ => fault(bridge),
    }
}

/// Status 0 transfers the message, 1 parks and retains it, and 2 returns it
/// intact because every receiver is closed. Fault returns 255.
///
/// # Safety
///
/// `bridge` follows `phoebe_typed_channel_new`. For nonzero `width`, `words`
/// points to that many readable u64 words until this call returns.
#[unsafe(export_name = "phoebe_rt_v1_send_typed_words")]
pub unsafe extern "C" fn phoebe_send_typed_words(
    bridge: *mut NativeCallContext,
    sender: i64,
    type_id: u64,
    words: *const u64,
    width: i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<TypedSendResult, ()> {
        if bridge.is_null() {
            return Err(());
        }
        let width = usize::try_from(width).map_err(|_| ())?;
        if width != 0 && words.is_null() {
            return Err(());
        }
        let words = if width == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(words, width) }
        };
        let token = Token(usize::try_from(sender).map_err(|_| ())?);
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        context
            .send_typed_words(&Sender { token }, type_id, words)
            .map_err(|_| ())
    }));
    match outcome {
        Ok(Ok(TypedSendResult::Sent)) => 0,
        Ok(Ok(TypedSendResult::Pending)) => 1,
        Ok(Ok(TypedSendResult::Closed)) => 2,
        _ => fault(bridge),
    }
}

/// Status 0 writes every message word. Status 1 parks and status 2 reports
/// End; neither writes any output. Fault returns 255.
///
/// # Safety
///
/// `bridge` follows `phoebe_typed_channel_new`. For nonzero `width`, `output`
/// points to that many aligned writable u64 words until this call returns.
#[unsafe(export_name = "phoebe_rt_v1_receive_typed_words")]
pub unsafe extern "C" fn phoebe_receive_typed_words(
    bridge: *mut NativeCallContext,
    receiver: i64,
    type_id: u64,
    output: *mut u64,
    width: i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<TypedReceiveResult, ()> {
        if bridge.is_null() {
            return Err(());
        }
        let width = usize::try_from(width).map_err(|_| ())?;
        if width != 0 && output.is_null() {
            return Err(());
        }
        let output = if width == 0 {
            &mut [][..]
        } else {
            unsafe { std::slice::from_raw_parts_mut(output, width) }
        };
        let token = Token(usize::try_from(receiver).map_err(|_| ())?);
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        context
            .receive_typed_words(&Receiver { token }, type_id, output)
            .map_err(|_| ())
    }));
    match outcome {
        Ok(Ok(TypedReceiveResult::Value)) => 0,
        Ok(Ok(TypedReceiveResult::Pending)) => 1,
        Ok(Ok(TypedReceiveResult::End)) => 2,
        _ => fault(bridge),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NativeTypedRoundTrip;

    impl Task for NativeTypedRoundTrip {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            let mut fault = false;
            let mut bridge = NativeCallContext {
                context: context as *mut _ as *mut c_void,
                fault: &mut fault,
                call_request: std::ptr::null_mut(),
            };
            let plan = TypedRightPlan::ReplyTo { part: 1 }.encode_native_words();
            let mut sender = -1;
            let mut receiver = -1;
            assert_eq!(
                unsafe {
                    phoebe_typed_channel_new(
                        &mut bridge,
                        1,
                        91,
                        2,
                        plan.as_ptr(),
                        plan.len() as i64,
                        &mut sender,
                        &mut receiver,
                    )
                },
                0
            );
            let (right, reply_receiver) = context.reply()?;
            let message = [42, right.token().0 as u64];
            assert_eq!(
                unsafe { phoebe_send_typed_words(&mut bridge, sender, 91, message.as_ptr(), 2) },
                0
            );
            let mut output = [0; 2];
            assert_eq!(
                unsafe {
                    phoebe_receive_typed_words(&mut bridge, receiver, 91, output.as_mut_ptr(), 2)
                },
                0
            );
            assert_eq!(output, message);
            context.close_reply_to(ReplyTo {
                token: ReplyToken(output[1] as usize),
            })?;
            context.cancel_reply(reply_receiver)?;
            context.close_sender(Sender {
                token: Token(sender as usize),
            })?;
            context.close_receiver(Receiver {
                token: Token(receiver as usize),
            })?;
            let mut closed_sender = -1;
            let mut closed_receiver = -1;
            assert_eq!(
                unsafe {
                    phoebe_typed_channel_new(
                        &mut bridge,
                        1,
                        91,
                        2,
                        plan.as_ptr(),
                        plan.len() as i64,
                        &mut closed_sender,
                        &mut closed_receiver,
                    )
                },
                0
            );
            context.close_receiver(Receiver {
                token: Token(closed_receiver as usize),
            })?;
            let (unsent_right, unsent_receiver) = context.reply()?;
            let unsent = [7, unsent_right.token().0 as u64];
            assert_eq!(
                unsafe {
                    phoebe_send_typed_words(&mut bridge, closed_sender, 91, unsent.as_ptr(), 2)
                },
                2
            );
            assert_eq!(unsent, [7, unsent_right.token().0 as u64]);
            context.close_reply_to(unsent_right)?;
            context.cancel_reply(unsent_receiver)?;
            context.close_sender(Sender {
                token: Token(closed_sender as usize),
            })?;
            assert!(!fault);
            Ok(Poll::Ready(output[0] as i64))
        }
    }

    #[test]
    fn native_typed_abi_moves_reply_right_through_queue() {
        let mut scheduler = Scheduler::with_limits(NativeTypedRoundTrip, ScopeLimits::default())
            .expect("root task");
        assert_eq!(scheduler.run().unwrap(), 42);
    }
}
