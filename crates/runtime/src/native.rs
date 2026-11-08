use super::*;

#[repr(C)]
pub struct NativeCallContext {
    pub context: *mut c_void,
    pub fault: *mut bool,
    /// Optional one-call mailbox supplied by a retained native task.
    pub call_request: *mut c_void,
}

pub(super) fn resource_code(error: ResourceError) -> i64 {
    error.native_word()
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

/// Fork kind: 0 sender, 1 receiver. Status 0 writes one new handle;
/// resource failure leaves the original handle owned by the caller.
///
/// # Safety
///
/// `bridge` must satisfy the lifetime and exclusive-borrow contract of
/// `phoebe_channel_new_i64`; `token` must be a checked handle of `kind` and
/// `out` must be aligned and writable for one i64.
#[unsafe(export_name = "phoebe_rt_v1_fork_handle_i64")]
pub unsafe extern "C" fn phoebe_fork_handle_i64(
    bridge: *mut NativeCallContext,
    token: i64,
    kind: i64,
    out: *mut i64,
) -> i64 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        let token = usize::try_from(token)
            .map(Token)
            .map_err(|_| "negative handle token")?;
        let result = match kind {
            0 => context
                .fork_sender_typed(&Sender { token })?
                .map(|handle| handle.token.0),
            1 => context
                .fork_receiver_typed(&Receiver { token })?
                .map(|handle| handle.token.0),
            _ => return Err("invalid handle kind".into()),
        };
        Ok::<_, String>(match result {
            Ok(fork) => {
                unsafe { *out = fork as i64 };
                0
            }
            Err(error) => resource_code(error),
        })
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

/// Register a cooperative yield and return Pending without changing
/// capability ownership. The generated frame already holds its resume PC.
///
/// # Safety
///
/// `bridge` must satisfy the lifetime and exclusive-borrow contract of
/// `phoebe_channel_new_i64`.
#[unsafe(export_name = "phoebe_rt_v1_yield")]
pub unsafe extern "C" fn phoebe_yield(bridge: *mut NativeCallContext) -> i64 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *bridge };
        let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
        context.yield_now()
    }));
    match outcome {
        Ok(Ok(())) => 1,
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
    children: Vec<NativeCallRequest>,
}

impl Drop for NativePollTask {
    fn drop(&mut self) {
        // A callee may retain an address into its caller's frame. Release
        // pending calls from the leaf inward before the root frame is freed.
        while let Some(child) = self.children.pop() {
            drop(child);
        }
    }
}

struct NativeCallRequest {
    entry: NativePollEntry,
    frame: Vec<i64>,
    parent_result_word: usize,
    result_words: usize,
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
        Ok(Self {
            entry,
            frame,
            children: Vec::new(),
        })
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
        loop {
            let mut fault = false;
            let mut request: Option<NativeCallRequest> = None;
            let mut bridge = NativeCallContext {
                context: context as *mut _ as *mut c_void,
                fault: &mut fault,
                call_request: (&mut request as *mut Option<NativeCallRequest>).cast(),
            };
            let status = if let Some(child) = self.children.last_mut() {
                (child.entry)(&mut bridge, child.frame.as_mut_ptr())
            } else {
                (self.entry)(&mut bridge, self.frame.as_mut_ptr())
            };
            if fault {
                return Err("native runtime ABI fault".into());
            }
            match status {
                0 if context.wait.is_none() && request.is_none() => {
                    if let Some(child) = self.children.pop() {
                        let parent = self
                            .children
                            .last_mut()
                            .map_or(&mut self.frame, |frame| &mut frame.frame);
                        parent[child.parent_result_word
                            ..child.parent_result_word + child.result_words]
                            .copy_from_slice(&child.frame[1..1 + child.result_words]);
                        continue;
                    }
                    return Ok(Poll::Ready(self.frame[1]));
                }
                0 => return Err("native poll returned Ready with a registered wait or call".into()),
                1 if request.is_none() => return Ok(Poll::Pending),
                3 if context.wait.is_none() => {
                    let child = request.ok_or("native poll requested call without child frame")?;
                    let parent_words = self
                        .children
                        .last()
                        .map_or(self.frame.len(), |frame| frame.frame.len());
                    if child.result_words + 1 > child.frame.len()
                        || child.parent_result_word == 0
                        || child
                            .parent_result_word
                            .checked_add(child.result_words)
                            .is_none_or(|end| end > parent_words)
                    {
                        return Err("native call result exceeds its frame".into());
                    }
                    self.children
                        .try_reserve(1)
                        .map_err(|_| "native call stack allocation failed")?;
                    self.children.push(child);
                }
                3 => return Err("native poll requested call with a registered wait".into()),
                255 => return Err("native poll function failed".into()),
                code => return Err(format!("invalid native poll status {code}")),
            }
        }
    }
}

/// Prepare one child frame for a same-task call. The caller must first save
/// its continuation PC and live values, then return poll status 3. The runtime
/// retains the parent frame, polls the child, and copies its result into the
/// specified parent words before resuming the parent.
///
/// # Safety
/// `bridge` must be provided by `NativePollTask::poll`; `entry` and `initial`
/// must satisfy the generated poll ABI. Initial words must be distinct and
/// within the child frame.
#[unsafe(export_name = "phoebe_rt_v1_call_poll")]
pub unsafe extern "C" fn phoebe_call_poll(
    bridge: *mut NativeCallContext,
    entry: NativePollEntry,
    frame_words: i64,
    initial: *const NativeFrameInit,
    initial_count: i64,
    parent_result_word: i64,
    result_words: i64,
) -> i64 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if bridge.is_null() {
            return Err(());
        }
        let bridge = unsafe { &mut *bridge };
        if bridge.call_request.is_null() {
            return Err(());
        }
        let mailbox = unsafe { &mut *(bridge.call_request as *mut Option<NativeCallRequest>) };
        if mailbox.is_some() {
            return Err(());
        }
        let words = usize::try_from(frame_words).map_err(|_| ())?;
        let count = usize::try_from(initial_count).map_err(|_| ())?;
        let parent_result_word = usize::try_from(parent_result_word).map_err(|_| ())?;
        let result_words = usize::try_from(result_words).map_err(|_| ())?;
        if words < 2
            || words
                .checked_mul(std::mem::size_of::<i64>())
                .is_none_or(|bytes| bytes > i32::MAX as usize)
            || count > words
            || (count != 0 && initial.is_null())
            || result_words + 1 > words
        {
            return Err(());
        }
        let initial = if count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(initial, count) }
        };
        let mut frame = Vec::new();
        frame.try_reserve_exact(words).map_err(|_| ())?;
        frame.resize(words, 0);
        for (index, arg) in initial.iter().enumerate() {
            let word = usize::try_from(arg.word).map_err(|_| ())?;
            if word == 0
                || word >= words
                || initial[..index].iter().any(|prior| prior.word == arg.word)
            {
                return Err(());
            }
            frame[word] = arg.value;
        }
        *mailbox = Some(NativeCallRequest {
            entry,
            frame,
            parent_result_word,
            result_words,
        });
        Ok(0)
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
        let transfer_count = usize::try_from(transfer_count).map_err(|_| ())?;
        if transfer_count > 4 || (transfer_count != 0 && transfers.is_null()) {
            return Err(());
        }
        let transfer_words = if transfer_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(transfers, transfer_count) }
        };
        let mut tokens = [CapabilityTransfer::Channel(Token(0)); 4];
        for (index, raw) in transfer_words.iter().enumerate() {
            tokens[index] =
                CapabilityTransfer::Channel(Token(usize::try_from(*raw).map_err(|_| ())?));
        }
        unsafe {
            spawn_poll_with_transfers(
                bridge,
                entry,
                frame_words,
                initial,
                initial_count,
                &tokens[..transfer_count],
            )
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

/// Shared admission path for scalar and typed native spawn descriptors.
///
/// # Safety
///
/// `bridge`, `entry`, and `initial` follow `phoebe_spawn_poll`'s contract.
pub(super) unsafe fn spawn_poll_with_transfers(
    bridge: *mut NativeCallContext,
    entry: NativePollEntry,
    frame_words: i64,
    initial: *const NativeFrameInit,
    initial_count: i64,
    transfers: &[CapabilityTransfer],
) -> Result<i64, ()> {
    if bridge.is_null() {
        return Err(());
    }
    let words = usize::try_from(frame_words).map_err(|_| ())?;
    let initial_count = usize::try_from(initial_count).map_err(|_| ())?;
    if words < 2
        || words
            .checked_mul(std::mem::size_of::<i64>())
            .is_none_or(|bytes| bytes > i32::MAX as usize)
        || initial_count > words
        || transfers.len() > words
        || (initial_count != 0 && initial.is_null())
    {
        return Err(());
    }
    let initial = if initial_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(initial, initial_count) }
    };
    let mut child = match unsafe { NativePollTask::new(entry, words) } {
        Ok(child) => child,
        Err(NativeFrameError::Allocation) => return Ok(resource_code(ResourceError::Allocation)),
        Err(NativeFrameError::InvalidLayout) => return Err(()),
    };
    for (index, arg) in initial.iter().enumerate() {
        let word = usize::try_from(arg.word).map_err(|_| ())?;
        if word == 0 || word >= words || initial[..index].iter().any(|prior| prior.word == arg.word)
        {
            return Err(());
        }
        child.frame_mut()[word] = arg.value;
    }
    let bridge = unsafe { &mut *bridge };
    let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
    match context.spawn_capabilities_owned(child, transfers) {
        Ok(_) => Ok(0),
        Err(error) => match error.resource_error {
            Some(resource_error) => Ok(resource_code(resource_error)),
            None => Err(()),
        },
    }
}

impl Task for StandaloneRootTask {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let mut fault = false;
        let mut bridge = NativeCallContext {
            context: context as *mut _ as *mut c_void,
            fault: &mut fault,
            call_request: std::ptr::null_mut(),
        };
        let value = (self.entry)(&mut bridge);
        if fault {
            Err("native runtime ABI fault".into())
        } else {
            Ok(Poll::Ready(value))
        }
    }
}

fn run_direct_root(entry: extern "C" fn(*mut NativeCallContext) -> i64) -> Result<i64, String> {
    let mut scheduler =
        Scheduler::with_limits(StandaloneRootTask { entry }, ScopeLimits::default())?;
    scheduler.run()
}

/// # Safety
///
/// `entry` must satisfy `NativePollTask::new` for `frame_words` words.
unsafe fn run_poll_root(entry: NativePollEntry, frame_words: i64) -> Result<i64, String> {
    let words = usize::try_from(frame_words).map_err(|_| "negative poll frame size")?;
    let root = unsafe { NativePollTask::new(entry, words) }.map_err(|error| error.to_string())?;
    let mut scheduler = Scheduler::with_limits(root, ScopeLimits::default())?;
    scheduler.run()
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
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| run_direct_root(entry)));
    unsafe { finish_root(outcome, result_out, std::ptr::null_mut(), 0) }
}

/// Copy a runtime failure into a caller-owned, NUL-terminated byte buffer.
///
/// # Safety
///
/// If `capacity` is nonzero, `out` must be writable for `capacity` bytes.
unsafe fn write_diagnostic(out: *mut u8, capacity: usize, message: &str) {
    if capacity == 0 || out.is_null() {
        return;
    }
    let mut count = message.len().min(capacity - 1);
    while !message.is_char_boundary(count) {
        count -= 1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(message.as_ptr(), out, count);
        *out.add(count) = 0;
    }
}

/// # Safety
///
/// `result_out` must be writable for one i64. If `error_capacity` is nonzero,
/// `error_out` must be writable for that many bytes.
unsafe fn finish_root(
    outcome: std::thread::Result<Result<i64, String>>,
    result_out: *mut i64,
    error_out: *mut u8,
    error_capacity: usize,
) -> i32 {
    match outcome {
        Ok(Ok(value)) => {
            unsafe { *result_out = value };
            0
        }
        Ok(Err(error)) => {
            unsafe { write_diagnostic(error_out, error_capacity, &error) };
            1
        }
        Err(_) => {
            unsafe { write_diagnostic(error_out, error_capacity, "runtime panicked") };
            1
        }
    }
}

/// Run a direct entry and return a bounded diagnostic to the host wrapper.
/// The original `phoebe_rt_v1_main` symbol remains available.
///
/// # Safety
///
/// `entry` and `result_out` obey `phoebe_runtime_main`'s contract. If
/// `error_capacity` is nonzero, `error_out` is writable for that many bytes.
#[unsafe(export_name = "phoebe_rt_v1_main_diagnostic")]
pub unsafe extern "C" fn phoebe_runtime_main_diagnostic(
    entry: extern "C" fn(*mut NativeCallContext) -> i64,
    result_out: *mut i64,
    error_out: *mut u8,
    error_capacity: usize,
) -> i32 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| run_direct_root(entry)));
    unsafe { finish_root(outcome, result_out, error_out, error_capacity) }
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
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        run_poll_root(entry, frame_words)
    }));
    unsafe { finish_root(outcome, result_out, std::ptr::null_mut(), 0) }
}

/// Run a generated poll entry and return a bounded diagnostic to the host
/// wrapper. The original `phoebe_rt_v1_main_poll` symbol remains available.
///
/// # Safety
///
/// `entry`, `frame_words`, and `result_out` obey
/// `phoebe_runtime_main_poll`'s contract. If `error_capacity` is nonzero,
/// `error_out` is writable for that many bytes.
#[unsafe(export_name = "phoebe_rt_v1_main_poll_diagnostic")]
pub unsafe extern "C" fn phoebe_runtime_main_poll_diagnostic(
    entry: NativePollEntry,
    frame_words: i64,
    result_out: *mut i64,
    error_out: *mut u8,
    error_capacity: usize,
) -> i32 {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        run_poll_root(entry, frame_words)
    }));
    unsafe { finish_root(outcome, result_out, error_out, error_capacity) }
}

#[cfg(test)]
mod native_poll_tests {
    use super::*;

    extern "C" fn synthetic_direct_fault(bridge: *mut NativeCallContext) -> i64 {
        unsafe { *(*bridge).fault = true };
        0
    }

    #[test]
    fn diagnostic_entry_reports_direct_fault_and_bounds_poll_error() {
        let mut value = 0;
        let mut message = [0u8; 96];
        assert_eq!(
            unsafe {
                phoebe_runtime_main_diagnostic(
                    synthetic_direct_fault,
                    &mut value,
                    message.as_mut_ptr(),
                    message.len(),
                )
            },
            1
        );
        let written = std::ffi::CStr::from_bytes_until_nul(&message).unwrap();
        assert!(
            written
                .to_str()
                .unwrap()
                .contains("native runtime ABI fault")
        );

        let mut short = [0u8; 8];
        assert_eq!(
            unsafe {
                phoebe_runtime_main_poll_diagnostic(
                    synthetic_poll,
                    -1,
                    &mut value,
                    short.as_mut_ptr(),
                    short.len(),
                )
            },
            1
        );
        assert_eq!(
            std::ffi::CStr::from_bytes_until_nul(&short)
                .unwrap()
                .to_bytes(),
            b"negativ"
        );
    }

    extern "C" fn synthetic_call_child(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 3) };
        if words[0] == 0 {
            words[0] = 1;
            let bridge = unsafe { &mut *bridge };
            let context = unsafe { &mut *(bridge.context as *mut Context<'_>) };
            context.yield_now().unwrap();
            return 1;
        }
        words[1] = words[2] + 1;
        0
    }

    extern "C" fn synthetic_call_parent(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 4) };
        if words[0] == 0 {
            words[0] = 1;
            words[3] += 1;
            let args = [NativeFrameInit { word: 2, value: 40 }];
            assert_eq!(
                unsafe {
                    phoebe_call_poll(bridge, synthetic_call_child, 3, args.as_ptr(), 1, 2, 1)
                },
                0
            );
            return 3;
        }
        assert_eq!(
            words[3], 1,
            "parent entry must not replay after child yields"
        );
        words[1] = words[2] + 1;
        0
    }

    #[test]
    fn native_call_retains_parent_and_child_frames_across_yield() {
        let root = unsafe { NativePollTask::new(synthetic_call_parent, 4) }.unwrap();
        let mut scheduler = Scheduler::new(root, 1, 0, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert_eq!(scheduler.stats().yields, 1);
        scheduler.verify().unwrap();
    }

    extern "C" fn synthetic_bad_call(bridge: *mut NativeCallContext, frame: *mut i64) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 2) };
        words[0] = 1;
        assert_eq!(
            unsafe { phoebe_call_poll(bridge, synthetic_call_child, 3, std::ptr::null(), 0, 2, 1) },
            0
        );
        3
    }

    #[test]
    fn native_call_rejects_result_outside_parent_frame() {
        let root = unsafe { NativePollTask::new(synthetic_bad_call, 2) }.unwrap();
        let mut scheduler = Scheduler::new(root, 1, 0, 0).unwrap();
        assert!(
            scheduler
                .run()
                .unwrap_err()
                .contains("result exceeds its frame")
        );
        scheduler.verify().unwrap();
    }

    extern "C" fn synthetic_receiving_child(
        bridge: *mut NativeCallContext,
        frame: *mut i64,
    ) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 3) };
        let status = unsafe { phoebe_receive_i64(bridge, words[2], &mut words[1]) };
        match status {
            0 => {
                assert_eq!(unsafe { phoebe_close_handle_i64(bridge, words[2], 1) }, 0);
                0
            }
            1 => 1,
            _ => 255,
        }
    }

    extern "C" fn synthetic_receiving_parent(
        bridge: *mut NativeCallContext,
        frame: *mut i64,
    ) -> i64 {
        let words = unsafe { std::slice::from_raw_parts_mut(frame, 4) };
        if words[0] == 0 {
            words[0] = 1;
            let args = [NativeFrameInit {
                word: 2,
                value: words[3],
            }];
            assert_eq!(
                unsafe {
                    phoebe_call_poll(bridge, synthetic_receiving_child, 3, args.as_ptr(), 1, 2, 1)
                },
                0
            );
            return 3;
        }
        words[1] = words[2];
        0
    }

    struct CallingHost {
        task: NativePollTask,
        started: bool,
    }

    impl Task for CallingHost {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            if !self.started {
                let (sender, receiver) = context.channel(1)?;
                self.task.write_token(3, receiver.token())?;
                let sender_token = sender.token();
                context.spawn(
                    Producer {
                        sender: Some(sender),
                    },
                    &[sender_token],
                )?;
                self.started = true;
            }
            self.task.poll(context)
        }
    }

    #[test]
    fn native_child_call_parks_on_channel_and_resumes_parent() {
        let task = unsafe { NativePollTask::new(synthetic_receiving_parent, 4) }.unwrap();
        let mut scheduler = Scheduler::new(
            CallingHost {
                task,
                started: false,
            },
            2,
            1,
            0,
        )
        .unwrap();
        assert_eq!(scheduler.run().unwrap(), 33);
        assert_eq!(scheduler.stats().parks, 1);
        assert_eq!(scheduler.stats().wakes, 1);
        scheduler.verify().unwrap();
    }

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
                call_request: std::ptr::null_mut(),
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
                    call_request: std::ptr::null_mut(),
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
