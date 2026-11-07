use super::*;

#[repr(C)]
pub struct NativeCallContext {
    pub context: *mut c_void,
    pub fault: *mut bool,
}

fn resource_code(error: ResourceError) -> i64 {
    match error {
        ResourceError::InvalidChannelCapacity => 1,
        ResourceError::ChannelLimit => 2,
        ResourceError::ReplyLimit => 3,
        ResourceError::Allocation => 4,
        ResourceError::TaskLimit => 5,
    }
}

/// Returns 0 and writes both handles on success, or a nonzero resource code
/// without writing either handle. The caller keeps `bridge` live until return.
///
/// # Safety
///
/// `bridge` must point to a live `NativeCallContext` with a uniquely borrowed
/// task `Context` and a live fault flag. Both output pointers must be distinct,
/// aligned, and writable for the duration of the call.
#[unsafe(export_name = "phoebe_rt_v1_channel_new_i64")]
pub unsafe extern "C" fn phoebe_channel_new_i64(
    bridge: *mut NativeCallContext,
    capacity: i64,
    sender_out: *mut i64,
    receiver_out: *mut i64,
) -> i64 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: generated code passes a live bridge and unique output slots
        // for the duration of this synchronous call.
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let capacity = match usize::try_from(capacity) {
            Ok(capacity) => capacity,
            Err(_) => return resource_code(ResourceError::InvalidChannelCapacity),
        };
        match context.channel_typed(capacity) {
            Ok((sender, receiver)) => {
                unsafe {
                    *sender_out = sender.token.0 as i64;
                    *receiver_out = receiver.token.0 as i64;
                }
                0
            }
            Err(error) => resource_code(error),
        }
    }));
    match outcome {
        Ok(code) => code,
        Err(_) => {
            if !bridge.is_null() {
                unsafe { *(*bridge).fault = true };
            }
            255
        }
    }
}

/// Close kind: 0 sender, 1 receiver. A fault is reported after native return.
///
/// # Safety
///
/// `bridge` must satisfy the same lifetime and exclusive-borrow contract as
/// `phoebe_channel_new_i64`; `token` and `kind` must come from checked Core IR.
#[unsafe(export_name = "phoebe_rt_v1_close_handle_i64")]
pub unsafe extern "C" fn phoebe_close_handle_i64(
    bridge: *mut NativeCallContext,
    token: i64,
    kind: i64,
) -> i64 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let token = usize::try_from(token)
            .map(Token)
            .map_err(|_| "negative handle token")?;
        match kind {
            0 => context.close_sender(Sender { token })?,
            1 => context.close_receiver(Receiver { token })?,
            _ => return Err("invalid handle kind".into()),
        }
        Ok::<_, String>(())
    }));
    match outcome {
        Ok(Ok(())) => 0,
        _ => {
            if !bridge.is_null() {
                unsafe { *(*bridge).fault = true };
            }
            255
        }
    }
}

/// Send status: 0 sent, 1 pending, 2 closed, 255 runtime fault. The sender
/// remains owned by the caller on every edge; a pending send retains its
/// message in the generated task frame and retries after a wake.
///
/// # Safety
///
/// `bridge` must satisfy the lifetime and exclusive-borrow contract of
/// `phoebe_channel_new_i64`; `sender_token` must be a checked sender token.
#[unsafe(export_name = "phoebe_rt_v1_send_i64")]
pub unsafe extern "C" fn phoebe_send_i64(
    bridge: *mut NativeCallContext,
    sender_token: i64,
    message: i64,
) -> i64 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let token = usize::try_from(sender_token)
            .map(Token)
            .map_err(|_| "negative sender token")?;
        context.send(&Sender { token }, message)
    }));
    match outcome {
        Ok(Ok(SendResult::Sent)) => 0,
        Ok(Ok(SendResult::Pending(_))) => 1,
        Ok(Ok(SendResult::Closed(_))) => 2,
        _ => {
            if !bridge.is_null() {
                unsafe { *(*bridge).fault = true };
            }
            255
        }
    }
}

/// Receive status: 0 value, 1 pending, 2 end, 255 runtime fault. `value_out`
/// is written only on the value edge. The receiver stays owned by the caller.
///
/// # Safety
///
/// `bridge` must satisfy the lifetime and exclusive-borrow contract of
/// `phoebe_channel_new_i64`; `receiver_token` must be a checked receiver
/// token, and `value_out` must be aligned and writable on a value edge.
#[unsafe(export_name = "phoebe_rt_v1_receive_i64")]
pub unsafe extern "C" fn phoebe_receive_i64(
    bridge: *mut NativeCallContext,
    receiver_token: i64,
    value_out: *mut i64,
) -> i64 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let token = usize::try_from(receiver_token)
            .map(Token)
            .map_err(|_| "negative receiver token")?;
        context.receive(&Receiver { token })
    }));
    match outcome {
        Ok(Ok(ReceiveResult::Value(value))) => {
            unsafe { *value_out = value };
            0
        }
        Ok(Ok(ReceiveResult::Pending)) => 1,
        Ok(Ok(ReceiveResult::End)) => 2,
        _ => {
            if !bridge.is_null() {
                unsafe { *(*bridge).fault = true };
            }
            255
        }
    }
}

struct StandaloneRootTask {
    entry: extern "C" fn(*mut NativeCallContext) -> i64,
}

/// Poll status: 0 ready, 1 pending, 255 fault. This scheduler task runs an
/// I64-returning poll function, which writes word 1 only on Ready. Word 0 is
/// its program counter; internal functions may reserve wider result areas.
pub type NativePollEntry = extern "C" fn(*mut NativeCallContext, *mut i64) -> i64;

/// One initialized word of a child frame. Generated code supplies exact
/// `entry_params` offsets from the verified physical frame layout.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NativeFrameInit {
    pub word: i64,
    pub value: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeFrameError {
    InvalidLayout,
    Allocation,
}

impl fmt::Display for NativeFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLayout => f.write_str("native poll frame requires a two-word header"),
            Self::Allocation => f.write_str("native poll frame allocation failed"),
        }
    }
}

pub struct NativePollTask {
    entry: NativePollEntry,
    frame: Vec<i64>,
}

impl NativePollTask {
    /// Allocate a stable, zeroed frame before any endpoint ownership transfer.
    ///
    /// # Safety
    ///
    /// `entry` must be verified I64-returning generated code using the v1 poll ABI and must
    /// access only the `frame_words` allocated words. It must preserve the
    /// frame's ownership invariants when returning Pending or Ready.
    pub unsafe fn new(
        entry: NativePollEntry,
        frame_words: usize,
    ) -> Result<Self, NativeFrameError> {
        if frame_words < 2 {
            return Err(NativeFrameError::InvalidLayout);
        }
        let mut frame = Vec::new();
        frame
            .try_reserve_exact(frame_words)
            .map_err(|_| NativeFrameError::Allocation)?;
        frame.resize(frame_words, 0);
        Ok(Self { entry, frame })
    }

    pub fn frame_mut(&mut self) -> &mut [i64] {
        &mut self.frame
    }

    /// Initialize a checked endpoint parameter before transferring its right
    /// into this task. The caller still performs the scheduler transfer.
    pub fn write_token(&mut self, word: usize, token: Token) -> Result<(), String> {
        let slot = self
            .frame
            .get_mut(word)
            .ok_or("native frame token slot is out of bounds")?;
        *slot = token.native_word()?;
        Ok(())
    }
}

impl Task for NativePollTask {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let mut fault = false;
        let mut bridge = NativeCallContext {
            context: context as *mut _ as *mut c_void,
            fault: &mut fault,
        };
        let status = (self.entry)(&mut bridge, self.frame.as_mut_ptr());
        if fault {
            return Err("native runtime ABI fault".into());
        }
        match status {
            0 if context.wait.is_none() => Ok(Poll::Ready(self.frame[1])),
            0 => Err("native poll returned Ready with a registered wait".into()),
            1 => Ok(Poll::Pending),
            255 => Err("native poll function failed".into()),
            code => Err(format!("invalid native poll status {code}")),
        }
    }
}

/// Admit a generated child only after its frame and arguments are complete.
/// Returns 0 on success, a resource code on a recoverable pre-commit failure,
/// or 255 on an invalid ABI call. Ownership transfers only on success.
///
/// # Safety
///
/// `bridge` must borrow a live task context. `entry` must be generated code
/// valid for `frame_words` and remain executable until the scope finishes.
/// `initial` and `transfers` must point to readable arrays of their declared
/// lengths for this call. Transfer tokens must name rights held by the caller.
#[unsafe(export_name = "phoebe_rt_v1_spawn_poll")]
pub unsafe extern "C" fn phoebe_spawn_poll(
    bridge: *mut NativeCallContext,
    entry: NativePollEntry,
    frame_words: i64,
    initial: *const NativeFrameInit,
    initial_count: i64,
    transfers: *const i64,
    transfer_count: i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if bridge.is_null() {
            return Err(());
        }
        let words = usize::try_from(frame_words).map_err(|_| ())?;
        let initial_count = usize::try_from(initial_count).map_err(|_| ())?;
        let transfer_count = usize::try_from(transfer_count).map_err(|_| ())?;
        if words < 2
            || words
                .checked_mul(std::mem::size_of::<i64>())
                .is_none_or(|bytes| bytes > i32::MAX as usize)
            || initial_count > words
            || transfer_count > 4
            || (initial_count != 0 && initial.is_null())
            || (transfer_count != 0 && transfers.is_null())
        {
            return Err(());
        }
        let initial = if initial_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(initial, initial_count) }
        };
        let transfer_words = if transfer_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(transfers, transfer_count) }
        };
        let mut tokens = [Token(0); 4];
        for (index, raw) in transfer_words.iter().enumerate() {
            tokens[index] = Token(usize::try_from(*raw).map_err(|_| ())?);
        }
        let mut child = match unsafe { NativePollTask::new(entry, words) } {
            Ok(child) => child,
            Err(NativeFrameError::Allocation) => {
                return Ok(resource_code(ResourceError::Allocation));
            }
            Err(NativeFrameError::InvalidLayout) => return Err(()),
        };
        for (index, arg) in initial.iter().enumerate() {
            let word = usize::try_from(arg.word).map_err(|_| ())?;
            if word == 0
                || word >= words
                || initial[..index].iter().any(|prior| prior.word == arg.word)
            {
                return Err(());
            }
            child.frame_mut()[word] = arg.value;
        }
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        match context.spawn_owned(child, &tokens[..transfer_count]) {
            Ok(_) => Ok(0),
            Err(error) => match error.resource_error {
                Some(resource_error) => Ok(resource_code(resource_error)),
                None => Err(()),
            },
        }
    }));
    match outcome {
        Ok(Ok(code)) => code,
        _ => {
            if !bridge.is_null() {
                unsafe { *(*bridge).fault = true };
            }
            255
        }
    }
}

impl Task for StandaloneRootTask {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let mut fault = false;
        let mut bridge = NativeCallContext {
            context: context as *mut _ as *mut c_void,
            fault: &mut fault,
        };
        let value = (self.entry)(&mut bridge);
        if fault {
            Err("native runtime ABI fault".into())
        } else {
            Ok(Poll::Ready(value))
        }
    }
}

/// Run a zero-argument compiled `main` under the default bounded scope.
///
/// # Safety
///
/// `entry` must be a verified Phoebe function with one hidden runtime-context
/// pointer and an `I64` return. `result_out` must be aligned and writable.
#[unsafe(export_name = "phoebe_rt_v1_main")]
pub unsafe extern "C" fn phoebe_runtime_main(
    entry: extern "C" fn(*mut NativeCallContext) -> i64,
    result_out: *mut i64,
) -> i32 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let limits = ScopeLimits::default();
        let mut scheduler = Scheduler::new(
            StandaloneRootTask { entry },
            limits.tasks,
            limits.channels,
            limits.replies,
        )?;
        scheduler.run()
    }));
    match outcome {
        Ok(Ok(value)) => {
            unsafe { *result_out = value };
            0
        }
        _ => 1,
    }
}

/// Run a zero-argument generated poll entry under the default bounded scope.
///
/// # Safety
///
/// `entry` must satisfy `NativePollTask::new` for `frame_words` words, which
/// include the two-word header. `result_out` must be aligned and writable.
#[unsafe(export_name = "phoebe_rt_v1_main_poll")]
pub unsafe extern "C" fn phoebe_runtime_main_poll(
    entry: NativePollEntry,
    frame_words: i64,
    result_out: *mut i64,
) -> i32 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let words = usize::try_from(frame_words).map_err(|_| "negative poll frame size")?;
        let root =
            unsafe { NativePollTask::new(entry, words) }.map_err(|error| error.to_string())?;
        let limits = ScopeLimits::default();
        let mut scheduler = Scheduler::new(root, limits.tasks, limits.channels, limits.replies)?;
        scheduler.run()
    }));
    match outcome {
        Ok(Ok(value)) => {
            unsafe { *result_out = value };
            0
        }
        _ => 1,
    }
}

#[cfg(test)]
mod native_poll_tests {
    use super::*;

    extern "C" fn synthetic_poll(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 4) };
        match words[0] {
            0 => {
                words[0] = 1;
                words[2] = 41;
                words[3] = frame as usize as i64;
                let bridge = unsafe { &mut *bridge };
                let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
                context.yield_now().expect("first poll may yield");
                1
            }
            1 => {
                assert_eq!(words[3], frame as usize as i64);
                words[1] = words[2] + 1;
                0
            }
            _ => 255,
        }
    }

    #[test]
    fn poll_abi_retains_a_stable_frame_across_scheduler_yield() {
        assert!(matches!(
            unsafe { NativePollTask::new(synthetic_poll, 1) },
            Err(NativeFrameError::InvalidLayout)
        ));
        assert!(matches!(
            unsafe { NativePollTask::new(synthetic_poll, usize::MAX) },
            Err(NativeFrameError::Allocation)
        ));
        let task = unsafe { NativePollTask::new(synthetic_poll, 4) }.unwrap();
        let mut scheduler = Scheduler::new(task, 1, 0, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert_eq!(scheduler.stats().yields, 1);
        scheduler.verify().unwrap();

        let mut result = -1;
        assert_eq!(
            unsafe { phoebe_runtime_main_poll(synthetic_poll, 4, &mut result) },
            0
        );
        assert_eq!(result, 42);
    }

    extern "C" fn synthetic_receive_poll(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 3) };
        let mut received = -1;
        match unsafe { phoebe_receive_i64(bridge, words[2], &mut received) } {
            1 => {
                assert_eq!(received, -1);
                words[0] = 1;
                1
            }
            0 => {
                assert_eq!(unsafe { phoebe_close_handle_i64(bridge, words[2], 1) }, 0);
                words[1] = received;
                0
            }
            _ => 255,
        }
    }

    #[derive(Default)]
    struct PollReceiverHost {
        sender: Option<Sender>,
        phase: u8,
    }

    impl Task for PollReceiverHost {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if self.phase == 0 {
                let (sender, receiver) = context.channel(1)?;
                let token = receiver.token();
                let mut child = unsafe { NativePollTask::new(synthetic_receive_poll, 3) }
                    .map_err(|error| error.to_string())?;
                child.frame_mut()[2] = token.0 as i64;
                context
                    .spawn_owned(child, &[token])
                    .map_err(|error| error.message)?;
                self.sender = Some(sender);
                self.phase = 1;
                context.yield_now()?;
                return Ok(Poll::Pending);
            }
            if self.phase == 1 {
                assert_eq!(
                    context.send(self.sender.as_ref().expect("sender"), 42)?,
                    SendResult::Sent
                );
                context.close_sender(self.sender.take().expect("sender"))?;
                self.phase = 2;
            }
            match context.join()? {
                Poll::Pending => Ok(Poll::Pending),
                Poll::Ready(()) => Ok(Poll::Ready(42)),
            }
        }
    }

    #[test]
    fn poll_abi_retries_receive_after_scheduler_wake() {
        let mut scheduler = Scheduler::new(PollReceiverHost::default(), 2, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert!(scheduler.stats().parks >= 1);
        assert!(scheduler.stats().wakes >= 1);
        scheduler.verify().unwrap();
    }

    extern "C" fn invalid_ready_with_wait(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        context.yield_now().expect("yield registers a wait");
        unsafe { *frame.add(1) = 1 };
        0
    }

    #[test]
    fn poll_abi_rejects_ready_with_a_registered_wait() {
        let task = unsafe { NativePollTask::new(invalid_ready_with_wait, 2) }.unwrap();
        let mut scheduler = Scheduler::new(task, 1, 0, 0).unwrap();
        assert!(
            scheduler
                .run()
                .unwrap_err()
                .contains("Ready with a registered wait")
        );
    }

    struct Drain {
        receiver: Option<Receiver>,
        values: Vec<i64>,
    }

    impl Task for Drain {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            loop {
                match context.receive(self.receiver.as_ref().expect("retained receiver"))? {
                    ReceiveResult::Value(value) => self.values.push(value),
                    ReceiveResult::Pending => return Ok(Poll::Pending),
                    ReceiveResult::End => {
                        context.close_receiver(self.receiver.take().expect("retained receiver"))?;
                        assert_eq!(self.values, [11, 22]);
                        return Ok(Poll::Ready(0));
                    }
                }
            }
        }
    }

    #[derive(Default)]
    struct AbiSender {
        sender: Option<Sender>,
        phase: u8,
    }

    impl Task for AbiSender {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if self.phase == 0 {
                let (sender, receiver) = context.channel(1)?;
                let token = receiver.token();
                context.spawn(
                    Drain {
                        receiver: Some(receiver),
                        values: Vec::new(),
                    },
                    &[token],
                )?;
                self.sender = Some(sender);
                self.phase = 1;
            }
            let mut fault = false;
            let mut bridge = NativeCallContext {
                context: context as *mut _ as *mut c_void,
                fault: &mut fault,
            };
            if self.phase == 1 {
                let token = self.sender.as_ref().expect("retained sender").token.0 as i64;
                assert_eq!(unsafe { phoebe_send_i64(&mut bridge, token, 11) }, 0);
                self.phase = 2;
            }
            if self.phase == 2 {
                let token = self.sender.as_ref().expect("retained sender").token.0 as i64;
                match unsafe { phoebe_send_i64(&mut bridge, token, 22) } {
                    1 => return Ok(Poll::Pending),
                    0 => {
                        self.phase = 3;
                        context.close_sender(self.sender.take().expect("retained sender"))?;
                    }
                    code => return Err(format!("unexpected send status {code}")),
                }
            }
            assert!(!fault);
            match context.join()? {
                Poll::Pending => Ok(Poll::Pending),
                Poll::Ready(()) => Ok(Poll::Ready(42)),
            }
        }
    }

    #[test]
    fn native_send_pending_retries_after_scheduler_wake() {
        let mut scheduler = Scheduler::new(AbiSender::default(), 2, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert!(scheduler.stats().parks >= 2);
        assert!(scheduler.stats().wakes >= 2);
        scheduler.verify().unwrap();
    }

    struct Producer {
        sender: Option<Sender>,
    }

    impl Task for Producer {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            match context.send(self.sender.as_ref().expect("retained sender"), 33)? {
                SendResult::Sent => {
                    context.close_sender(self.sender.take().expect("retained sender"))?;
                    Ok(Poll::Ready(0))
                }
                SendResult::Pending(_) => Ok(Poll::Pending),
                SendResult::Closed(_) => Err("receiver closed early".into()),
            }
        }
    }

    #[derive(Default)]
    struct AbiReceiver {
        receiver: Option<Receiver>,
        received: bool,
    }

    impl Task for AbiReceiver {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if self.receiver.is_none() && !self.received {
                let (sender, receiver) = context.channel(1)?;
                let token = sender.token();
                context.spawn(
                    Producer {
                        sender: Some(sender),
                    },
                    &[token],
                )?;
                self.receiver = Some(receiver);
            }
            if let Some(receiver) = &self.receiver {
                let mut fault = false;
                let mut bridge = NativeCallContext {
                    context: context as *mut _ as *mut c_void,
                    fault: &mut fault,
                };
                let mut value = -1;
                match unsafe {
                    phoebe_receive_i64(&mut bridge, receiver.token.0 as i64, &mut value)
                } {
                    1 => {
                        assert_eq!(value, -1);
                        return Ok(Poll::Pending);
                    }
                    0 => assert_eq!(value, 33),
                    code => return Err(format!("unexpected receive status {code}")),
                }
                assert!(!fault);
                context.close_receiver(self.receiver.take().expect("retained receiver"))?;
                self.received = true;
            }
            match context.join()? {
                Poll::Pending => Ok(Poll::Pending),
                Poll::Ready(()) => Ok(Poll::Ready(33)),
            }
        }
    }

    #[test]
    fn native_receive_pending_writes_only_after_scheduler_wake() {
        let mut scheduler = Scheduler::new(AbiReceiver::default(), 2, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 33);
        assert!(scheduler.stats().parks >= 1);
        assert!(scheduler.stats().wakes >= 1);
        scheduler.verify().unwrap();
    }
}
