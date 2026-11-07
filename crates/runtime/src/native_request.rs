//! Concrete native ABI for a request carrying one I64 and one reply right.

use super::*;

fn fault(bridge: *mut NativeCallContext) -> i64 {
    if !bridge.is_null() {
        // SAFETY: generated callers supply a live bridge and fault flag.
        unsafe { *(*bridge).fault = true };
    }
    255
}

/// Status 0 creates both request-channel handles. Resource failure publishes
/// neither output. The request channel has the same handle budget as an I64
/// channel, but accepts only the two-word request layout below.
///
/// # Safety
///
/// `bridge` must point to a live native call context with exclusive access to
/// its task context. Output pointers must be distinct, aligned writable I64
/// slots for the duration of this call.
#[unsafe(export_name = "phoebe_rt_v1_request_channel_new_i64")]
pub unsafe extern "C" fn phoebe_request_channel_new_i64(
    bridge: *mut NativeCallContext,
    capacity: i64,
    sender_out: *mut i64,
    receiver_out: *mut i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let capacity = match usize::try_from(capacity) {
            Ok(capacity) => capacity,
            Err(_) => return native::resource_code(ResourceError::InvalidChannelCapacity),
        };
        match context.request_channel_typed(capacity) {
            Ok((sender, receiver)) => {
                unsafe {
                    *sender_out = sender.token().native_word().expect("bounded handle token");
                    *receiver_out = receiver
                        .token()
                        .native_word()
                        .expect("bounded handle token");
                }
                0
            }
            Err(error) => native::resource_code(error),
        }
    }));
    outcome.unwrap_or_else(|_| fault(bridge))
}

/// Status 0 transfers the complete request into the queue. Status 1 parks
/// while retaining both words in the caller's frame. Status 2 reports a
/// closed receiver and leaves the request intact with the caller. A runtime
/// fault returns 255; the scheduler then closes task-owned rights.
///
/// # Safety
///
/// `bridge` must satisfy `phoebe_request_channel_new_i64`'s contract.
/// `sender` and `right` must be live capabilities owned by the current task.
#[unsafe(export_name = "phoebe_rt_v1_send_request_i64")]
pub unsafe extern "C" fn phoebe_send_request_i64(
    bridge: *mut NativeCallContext,
    sender: i64,
    value: i64,
    right: i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let sender = Token(usize::try_from(sender).map_err(|_| "negative sender token")?);
        let right = ReplyToken(usize::try_from(right).map_err(|_| "negative reply token")?);
        let request = Request {
            value,
            reply: ReplyTo { token: right },
        };
        context
            .send_request_owned(&Sender { token: sender }, request)
            .map_err(|error| error.message)
    }));
    match outcome {
        Ok(Ok(RequestSendResult::Sent)) => 0,
        Ok(Ok(RequestSendResult::Pending(_))) => 1,
        Ok(Ok(RequestSendResult::Closed(_))) => 2,
        _ => fault(bridge),
    }
}

/// Status 0 writes both request words and transfers the nested reply right
/// from the queue to the current task. Status 1 parks, writing neither word;
/// status 2 reports End, also without writing either word.
///
/// # Safety
///
/// `bridge` must satisfy `phoebe_request_channel_new_i64`'s contract.
/// `receiver` must be a live request-channel receiver owned by the current
/// task. Outputs must be distinct, aligned writable I64 slots.
#[unsafe(export_name = "phoebe_rt_v1_receive_request_i64")]
pub unsafe extern "C" fn phoebe_receive_request_i64(
    bridge: *mut NativeCallContext,
    receiver: i64,
    value_out: *mut i64,
    right_out: *mut i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let receiver = Token(usize::try_from(receiver).map_err(|_| "negative receiver token")?);
        context.receive_request(&Receiver { token: receiver })
    }));
    match outcome {
        Ok(Ok(RequestReceiveResult::Value(request))) => {
            let right = i64::try_from(request.reply.token().0).expect("bounded reply token");
            unsafe {
                *value_out = request.value;
                *right_out = right;
            }
            0
        }
        Ok(Ok(RequestReceiveResult::Pending)) => 1,
        Ok(Ok(RequestReceiveResult::End)) => 2,
        _ => fault(bridge),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Poll;

    extern "C" fn request_round_trip(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 8) };
        if unsafe { phoebe_request_channel_new_i64(bridge, 1, &mut words[2], &mut words[3]) } != 0 {
            return 255;
        }
        if unsafe { phoebe_reply_new_i64(bridge, &mut words[4], &mut words[5]) } != 0 {
            return 255;
        }
        if unsafe { phoebe_send_request_i64(bridge, words[2], 42, words[4]) } != 0 {
            return 255;
        }
        words[6] = -1;
        words[7] = -1;
        if unsafe { phoebe_receive_request_i64(bridge, words[3], &mut words[6], &mut words[7]) }
            != 0
        {
            return 255;
        }
        if unsafe { phoebe_close_handle_i64(bridge, words[2], 0) } != 0
            || unsafe { phoebe_close_handle_i64(bridge, words[3], 1) } != 0
            || unsafe { phoebe_reply_complete_i64(bridge, words[7], words[6]) } != 0
        {
            return 255;
        }
        let mut answer = -1;
        if unsafe { phoebe_reply_receive_i64(bridge, words[5], &mut answer) } != 0 {
            return 255;
        }
        words[1] = answer;
        0
    }

    #[test]
    fn native_request_round_trip_transfers_nested_reply_right() {
        let task = unsafe { NativePollTask::new(request_round_trip, 8) }.unwrap();
        let mut scheduler = Scheduler::new(task, 1, 1, 1).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        scheduler.verify().unwrap();
    }

    extern "C" fn closed_request(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 6) };
        if unsafe { phoebe_request_channel_new_i64(bridge, 1, &mut words[2], &mut words[3]) } != 0
            || unsafe { phoebe_reply_new_i64(bridge, &mut words[4], &mut words[5]) } != 0
            || unsafe { phoebe_close_handle_i64(bridge, words[3], 1) } != 0
        {
            return 255;
        }
        assert_eq!(
            unsafe { phoebe_send_request_i64(bridge, words[2], 42, words[4]) },
            2
        );
        if unsafe { phoebe_reply_complete_i64(bridge, words[4], 42) } != 0
            || unsafe { phoebe_close_handle_i64(bridge, words[2], 0) } != 0
        {
            return 255;
        }
        let mut answer = -1;
        if unsafe { phoebe_reply_receive_i64(bridge, words[5], &mut answer) } != 0 {
            return 255;
        }
        words[1] = answer;
        0
    }

    #[test]
    fn native_closed_send_keeps_request_right_with_caller() {
        let task = unsafe { NativePollTask::new(closed_request, 6) }.unwrap();
        let mut scheduler = Scheduler::new(task, 1, 1, 1).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        scheduler.verify().unwrap();
    }

    extern "C" fn channel_limit(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 4) };
        words[2] = 17;
        words[3] = 19;
        words[1] =
            unsafe { phoebe_request_channel_new_i64(bridge, 1, &mut words[2], &mut words[3]) };
        assert_eq!((words[2], words[3]), (17, 19));
        0
    }

    #[test]
    fn native_request_creation_failure_publishes_no_handles() {
        let task = unsafe { NativePollTask::new(channel_limit, 4) }.unwrap();
        let mut scheduler = Scheduler::new(task, 1, 0, 0).unwrap();
        assert_eq!(
            scheduler.run().unwrap(),
            native::resource_code(ResourceError::ChannelLimit)
        );
        scheduler.verify().unwrap();
    }

    extern "C" fn waiting_service(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 5) };
        words[3] = -1;
        words[4] = -1;
        match unsafe { phoebe_receive_request_i64(bridge, words[2], &mut words[3], &mut words[4]) }
        {
            1 => {
                assert_eq!((words[3], words[4]), (-1, -1));
                1
            }
            0 => {
                if unsafe { phoebe_reply_complete_i64(bridge, words[4], words[3]) } != 0
                    || unsafe { phoebe_close_handle_i64(bridge, words[2], 1) } != 0
                {
                    return 255;
                }
                words[1] = 0;
                0
            }
            _ => 255,
        }
    }

    #[derive(Default)]
    struct WaitingRoot {
        phase: u8,
        sender: Option<Sender>,
        right: Option<ReplyTo>,
        receiver: Option<ReplyReceiver>,
    }

    impl Task for WaitingRoot {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if self.phase == 0 {
                let (sender, receiver) = context.request_channel(1)?;
                let (right, reply_receiver) = context.reply()?;
                let transfer = CapabilityTransfer::Channel(receiver.token());
                let mut child = unsafe { NativePollTask::new(waiting_service, 5) }
                    .map_err(|error| error.to_string())?;
                child.frame_mut()[2] = receiver.token().native_word()?;
                context
                    .spawn_capabilities_owned(child, &[transfer])
                    .map_err(|error| error.message)?;
                self.sender = Some(sender);
                self.right = Some(right);
                self.receiver = Some(reply_receiver);
                self.phase = 1;
                context.yield_now()?;
                return Ok(Poll::Pending);
            }
            if self.phase == 1 {
                let sender = self.sender.take().expect("sender");
                let right = self.right.take().expect("right");
                assert!(matches!(
                    context.send_request(
                        &sender,
                        Request {
                            value: 42,
                            reply: right
                        }
                    )?,
                    RequestSendResult::Sent
                ));
                context.close_sender(sender)?;
                self.phase = 2;
            }
            match context.receive_reply(self.receiver.as_ref().expect("reply receiver"))? {
                ReplyReceiveResult::Pending => Ok(Poll::Pending),
                ReplyReceiveResult::Value(value) => {
                    self.receiver = None;
                    Ok(Poll::Ready(value))
                }
                ReplyReceiveResult::End => Err("service abandoned reply".into()),
            }
        }
    }

    #[test]
    fn native_request_receive_parks_without_writing_then_wakes() {
        let mut scheduler = Scheduler::new(WaitingRoot::default(), 2, 1, 1).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert!(scheduler.stats().parks >= 2);
        scheduler.verify().unwrap();
    }
}
