//! One-shot reply operations for generated native poll functions.

use super::*;

/// Native transfer descriptor: 0 channel, 1 reply right, 2 reply receiver.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NativeCapabilityTransfer {
    pub kind: i64,
    pub token: i64,
}

/// Admit a child with a mixed list of channel and reply capabilities.
/// Status 0 commits every transfer; a resource code or fault commits none.
///
/// # Safety
///
/// `bridge`, `entry`, `frame_words`, and `initial` follow
/// `phoebe_spawn_poll`'s contract. `transfers` must point to a readable array
/// of `transfer_count` descriptors, each owned by the calling task.
#[unsafe(export_name = "phoebe_rt_v1_spawn_poll_caps")]
pub unsafe extern "C" fn phoebe_spawn_poll_caps(
    bridge: *mut NativeCallContext,
    entry: NativePollEntry,
    frame_words: i64,
    initial: *const NativeFrameInit,
    initial_count: i64,
    transfers: *const NativeCapabilityTransfer,
    transfer_count: i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let count = usize::try_from(transfer_count).map_err(|_| ())?;
        let words = usize::try_from(frame_words).map_err(|_| ())?;
        if count > words || (count != 0 && transfers.is_null()) {
            return Err(());
        }
        let descriptors = if count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(transfers, count) }
        };
        let mut decoded = Vec::new();
        if decoded.try_reserve_exact(count).is_err() {
            return Ok(native::resource_code(ResourceError::Allocation));
        }
        for descriptor in descriptors {
            let token = usize::try_from(descriptor.token).map_err(|_| ())?;
            decoded.push(match descriptor.kind {
                0 => CapabilityTransfer::Channel(Token(token)),
                1 => CapabilityTransfer::ReplyRight(ReplyToken(token)),
                2 => CapabilityTransfer::ReplyReceiver(ReplyReceiverToken(token)),
                _ => return Err(()),
            });
        }
        unsafe {
            native::spawn_poll_with_transfers(
                bridge,
                entry,
                frame_words,
                initial,
                initial_count,
                &decoded,
            )
        }
    }));
    match outcome {
        Ok(Ok(code)) => code,
        _ => fault(bridge),
    }
}

fn fault(bridge: *mut NativeCallContext) -> i64 {
    if !bridge.is_null() {
        // SAFETY: a live bridge carries a live fault flag for the poll call.
        unsafe { *(*bridge).fault = true };
    }
    255
}

/// Status 0 writes both rights; resource failure leaves both outputs untouched.
///
/// # Safety
///
/// `bridge` must point to a live native call context with a uniquely borrowed
/// task context. Outputs must be distinct, aligned, and writable i64 slots.
#[unsafe(export_name = "phoebe_rt_v1_reply_new_i64")]
pub unsafe extern "C" fn phoebe_reply_new_i64(
    bridge: *mut NativeCallContext,
    right_out: *mut i64,
    receiver_out: *mut i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        match context.reply_typed() {
            Ok((right, receiver)) => {
                unsafe {
                    *right_out = right.token.0 as i64;
                    *receiver_out = receiver.slot as i64;
                }
                0
            }
            Err(error) => native::resource_code(error),
        }
    }));
    outcome.unwrap_or_else(|_| fault(bridge))
}

/// Status 0 delivers; 2 means the receiver was canceled. Both consume the
/// right. Status 255 is an internal fault.
///
/// # Safety
///
/// `bridge` must satisfy `phoebe_reply_new_i64`'s bridge contract. `right`
/// must be a live right owned by the current task.
#[unsafe(export_name = "phoebe_rt_v1_reply_complete_i64")]
pub unsafe extern "C" fn phoebe_reply_complete_i64(
    bridge: *mut NativeCallContext,
    right: i64,
    value: i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let token = ReplyToken(usize::try_from(right).map_err(|_| "negative reply token")?);
        context.complete(ReplyTo { token }, value)
    }));
    match outcome {
        Ok(Ok(CompleteResult::Delivered)) => 0,
        Ok(Ok(CompleteResult::Closed(_))) => 2,
        _ => fault(bridge),
    }
}

/// Status 0 writes a value and consumes the receiver; 1 parks the task and
/// retains the receiver; 2 consumes it on end. Status 255 is an internal fault.
///
/// # Safety
///
/// `bridge` must satisfy `phoebe_reply_new_i64`'s bridge contract. `receiver`
/// must be owned by the current task; `value_out` must be aligned and writable
/// on the value edge.
#[unsafe(export_name = "phoebe_rt_v1_reply_receive_i64")]
pub unsafe extern "C" fn phoebe_reply_receive_i64(
    bridge: *mut NativeCallContext,
    receiver: i64,
    value_out: *mut i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let slot = usize::try_from(receiver).map_err(|_| "negative reply receiver")?;
        context.receive_reply(&ReplyReceiver { slot })
    }));
    match outcome {
        Ok(Ok(ReplyReceiveResult::Value(value))) => {
            unsafe { *value_out = value };
            0
        }
        Ok(Ok(ReplyReceiveResult::Pending)) => 1,
        Ok(Ok(ReplyReceiveResult::End)) => 2,
        _ => fault(bridge),
    }
}

/// Kind 0 closes the reply right; kind 1 cancels the receiver.
///
/// # Safety
///
/// `bridge` must satisfy `phoebe_reply_new_i64`'s bridge contract. `token`
/// must name a capability of `kind` owned by the current task.
#[unsafe(export_name = "phoebe_rt_v1_reply_close_i64")]
pub unsafe extern "C" fn phoebe_reply_close_i64(
    bridge: *mut NativeCallContext,
    token: i64,
    kind: i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let token = usize::try_from(token).map_err(|_| "negative reply capability")?;
        match kind {
            0 => context.close_reply_to(ReplyTo {
                token: ReplyToken(token),
            }),
            1 => context.cancel_reply(ReplyReceiver { slot: token }),
            _ => Err("invalid reply capability kind".into()),
        }
    }));
    match outcome {
        Ok(Ok(())) => 0,
        _ => fault(bridge),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn complete_and_receive(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 4) };
        if unsafe { phoebe_reply_new_i64(bridge, &mut words[2], &mut words[3]) } != 0 {
            return 255;
        }
        if unsafe { phoebe_reply_complete_i64(bridge, words[2], 42) } != 0 {
            return 255;
        }
        let mut value = -1;
        if unsafe { phoebe_reply_receive_i64(bridge, words[3], &mut value) } != 0 {
            return 255;
        }
        words[1] = value;
        0
    }

    #[test]
    fn native_reply_pair_completes_once() {
        let task = unsafe { NativePollTask::new(complete_and_receive, 4) }.unwrap();
        let mut scheduler = Scheduler::new(task, 1, 0, 1).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        scheduler.verify().unwrap();
    }

    extern "C" fn cancel_then_complete(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 4) };
        if unsafe { phoebe_reply_new_i64(bridge, &mut words[2], &mut words[3]) } != 0 {
            return 255;
        }
        assert_eq!(unsafe { phoebe_reply_close_i64(bridge, words[3], 1) }, 0);
        words[1] = unsafe { phoebe_reply_complete_i64(bridge, words[2], 42) };
        0
    }

    #[test]
    fn native_cancellation_returns_closed() {
        let task = unsafe { NativePollTask::new(cancel_then_complete, 4) }.unwrap();
        let mut scheduler = Scheduler::new(task, 1, 0, 1).unwrap();
        assert_eq!(scheduler.run().unwrap(), 2);
        scheduler.verify().unwrap();
    }

    extern "C" fn reply_limit(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 4) };
        words[2] = 17;
        words[3] = 19;
        words[1] = unsafe { phoebe_reply_new_i64(bridge, &mut words[2], &mut words[3]) };
        assert_eq!((words[2], words[3]), (17, 19));
        0
    }

    #[test]
    fn native_reply_limit_does_not_publish_partial_pair() {
        let task = unsafe { NativePollTask::new(reply_limit, 4) }.unwrap();
        let mut scheduler = Scheduler::new(task, 1, 0, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 3);
        scheduler.verify().unwrap();
    }

    extern "C" fn receive_after_wake(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 3) };
        let mut value = -1;
        match unsafe { phoebe_reply_receive_i64(bridge, words[2], &mut value) } {
            1 => {
                assert_eq!(value, -1);
                1
            }
            0 if value == 42 => {
                words[1] = value;
                0
            }
            _ => 255,
        }
    }

    #[derive(Default)]
    struct ReplyingHost {
        right: Option<ReplyTo>,
        phase: u8,
    }

    impl Task for ReplyingHost {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if self.phase == 0 {
                let (right, receiver) = context.reply()?;
                let transfer = receiver.transfer();
                let mut child = unsafe { NativePollTask::new(receive_after_wake, 3) }
                    .map_err(|error| error.to_string())?;
                child.frame_mut()[2] = receiver.slot as i64;
                context
                    .spawn_capabilities_owned(child, &[transfer])
                    .map_err(|error| error.message)?;
                self.right = Some(right);
                self.phase = 1;
                context.yield_now()?;
                return Ok(Poll::Pending);
            }
            if self.phase == 1 {
                assert_eq!(
                    context.complete(self.right.take().expect("right"), 42)?,
                    CompleteResult::Delivered
                );
                self.phase = 2;
            }
            match context.join()? {
                Poll::Pending => Ok(Poll::Pending),
                Poll::Ready(()) => Ok(Poll::Ready(42)),
            }
        }
    }

    #[test]
    fn native_reply_receive_parks_and_wakes_after_transfer() {
        let mut scheduler = Scheduler::new(ReplyingHost::default(), 2, 0, 1).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert!(scheduler.stats().parks >= 1);
        assert!(scheduler.stats().wakes >= 1);
        scheduler.verify().unwrap();
    }

    extern "C" fn native_reply_child(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 3) };
        if unsafe { phoebe_reply_complete_i64(bridge, words[2], 42) } != 0 {
            return 255;
        }
        words[1] = 0;
        0
    }

    extern "C" fn native_spawn_reply(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 4) };
        if words[0] == 0 {
            if unsafe { phoebe_reply_new_i64(bridge, &mut words[2], &mut words[3]) } != 0 {
                return 255;
            }
            let initial = NativeFrameInit {
                word: 2,
                value: words[2],
            };
            let transfer = NativeCapabilityTransfer {
                kind: 1,
                token: words[2],
            };
            if unsafe {
                phoebe_spawn_poll_caps(bridge, native_reply_child, 3, &initial, 1, &transfer, 1)
            } != 0
            {
                return 255;
            }
            words[0] = 1;
        }
        let mut value = -1;
        match unsafe { phoebe_reply_receive_i64(bridge, words[3], &mut value) } {
            1 => 1,
            0 => {
                words[1] = value;
                0
            }
            _ => 255,
        }
    }

    #[test]
    fn native_spawn_transfers_reply_right_to_native_child() {
        let task = unsafe { NativePollTask::new(native_spawn_reply, 4) }.unwrap();
        let mut scheduler = Scheduler::new(task, 2, 0, 1).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        scheduler.verify().unwrap();
    }

    extern "C" fn native_rejected_spawn(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 4) };
        if unsafe { phoebe_reply_new_i64(bridge, &mut words[2], &mut words[3]) } != 0 {
            return 255;
        }
        let initial = NativeFrameInit {
            word: 2,
            value: words[2],
        };
        let transfer = NativeCapabilityTransfer {
            kind: 1,
            token: words[2],
        };
        if unsafe {
            phoebe_spawn_poll_caps(bridge, native_reply_child, 3, &initial, 1, &transfer, 1)
        } != 5
        {
            return 255;
        }
        if unsafe { phoebe_reply_complete_i64(bridge, words[2], 42) } != 0 {
            return 255;
        }
        let mut value = -1;
        if unsafe { phoebe_reply_receive_i64(bridge, words[3], &mut value) } != 0 {
            return 255;
        }
        words[1] = value;
        0
    }

    #[test]
    fn native_rejected_spawn_retains_reply_right() {
        let task = unsafe { NativePollTask::new(native_rejected_spawn, 4) }.unwrap();
        let mut scheduler = Scheduler::new(task, 1, 0, 1).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        scheduler.verify().unwrap();
    }
}
