//! Scope-owned cooperative task, channel, and one-shot reply kernel.
//! Compiler-generated continuations and typed message layouts are future work.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::Poll;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskId(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Token(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReplyToken(usize);

#[derive(Debug)]
pub struct ReplyTo {
    token: ReplyToken,
}

impl ReplyTo {
    pub fn token(&self) -> ReplyToken {
        self.token
    }
}

#[derive(Debug)]
pub struct ReplyReceiver {
    slot: usize,
}

#[derive(Debug)]
pub struct Request {
    pub value: i64,
    pub reply: ReplyTo,
}

#[derive(Debug)]
pub enum RequestSendResult {
    Sent,
    Pending(Request),
    Closed(Request),
}

#[derive(Debug)]
/// A pre-commit runtime error that returns ownership of its input unchanged.
pub struct OwnedError<T> {
    pub message: String,
    pub resource_error: Option<ResourceError>,
    pub value: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceError {
    InvalidChannelCapacity,
    ChannelLimit,
    ReplyLimit,
    TaskLimit,
    Allocation,
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidChannelCapacity | Self::ChannelLimit => {
                f.write_str("channel capacity or scope channel limit exceeded")
            }
            Self::ReplyLimit => f.write_str("scope reply limit exceeded"),
            Self::TaskLimit => f.write_str("scope task limit exceeded"),
            Self::Allocation => f.write_str("scope allocation failed"),
        }
    }
}

impl std::error::Error for ResourceError {}

/// A synchronous native call borrows the current task context for its duration.
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

#[derive(Debug)]
pub enum RequestReceiveResult {
    Value(Request),
    Pending,
    End,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompleteResult {
    Delivered,
    Closed(i64),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReplyReceiveResult {
    Value(i64),
    Pending,
    End,
}

#[derive(Debug)]
pub struct Sender {
    token: Token,
}

impl Sender {
    pub fn token(&self) -> Token {
        self.token
    }

    pub(crate) fn from_token(token: Token) -> Self {
        Self { token }
    }
}

#[derive(Debug)]
pub struct Receiver {
    token: Token,
}

impl Receiver {
    pub fn token(&self) -> Token {
        self.token
    }

    pub(crate) fn from_token(token: Token) -> Self {
        Self { token }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendResult {
    Sent,
    Pending(i64),
    Closed(i64),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReceiveResult {
    Value(i64),
    Pending,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStats {
    pub tasks_created: usize,
    pub parks: usize,
    pub yields: usize,
    pub wakes: usize,
    pub discarded_messages: usize,
    pub max_queued: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeLimits {
    pub tasks: usize,
    pub channels: usize,
    pub replies: usize,
}

impl Default for ScopeLimits {
    fn default() -> Self {
        Self {
            tasks: 64,
            channels: 64,
            replies: 0,
        }
    }
}

pub trait Task {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandleKind {
    Sender,
    Receiver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChannelKind {
    Scalar,
    Request,
}

#[derive(Debug)]
enum Message {
    Scalar(i64),
    Request(Request),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplyOwner {
    Task(TaskId),
    Queue(usize),
    Closed,
}

struct ReplyRight {
    slot: usize,
    owner: ReplyOwner,
}

struct ReplySlot {
    right: ReplyToken,
    receiver: Option<TaskId>,
    value: Option<i64>,
    waiters: VecDeque<TaskId>,
}

#[derive(Clone, Copy, Debug)]
struct Handle {
    channel: usize,
    kind: HandleKind,
    owner: Option<TaskId>,
}

struct Channel {
    queue: VecDeque<Message>,
    kind: ChannelKind,
    capacity: usize,
    senders: usize,
    receivers: usize,
    send_waiters: VecDeque<TaskId>,
    receive_waiters: VecDeque<TaskId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    Send(usize),
    Receive(usize),
    Reply(usize),
    Join,
    Yield,
}

#[derive(Debug)]
enum TaskState {
    Runnable,
    Waiting(Wait),
    Done(i64),
    Failed(String),
}

struct TaskRecord {
    body: Option<Box<dyn Task>>,
    state: TaskState,
}

pub struct Scheduler {
    tasks: Vec<TaskRecord>,
    ready: VecDeque<TaskId>,
    channels: Vec<Channel>,
    handles: Vec<Handle>,
    reply_rights: Vec<ReplyRight>,
    reply_slots: Vec<ReplySlot>,
    max_tasks: usize,
    max_channels: usize,
    max_replies: usize,
    stats: RuntimeStats,
}

pub struct Context<'a> {
    scheduler: &'a mut Scheduler,
    task: TaskId,
    wait: Option<Wait>,
}

impl Scheduler {
    pub fn new(
        root: impl Task + 'static,
        max_tasks: usize,
        max_channels: usize,
        max_replies: usize,
    ) -> Result<Self, String> {
        if max_tasks == 0 {
            return Err("scope requires capacity for its root task".into());
        }
        let mut tasks = Vec::new();
        tasks
            .try_reserve_exact(max_tasks)
            .map_err(|error| error.to_string())?;
        tasks.push(TaskRecord {
            body: Some(Box::new(root)),
            state: TaskState::Runnable,
        });
        let mut ready = VecDeque::new();
        ready
            .try_reserve_exact(max_tasks)
            .map_err(|error| error.to_string())?;
        ready.push_back(TaskId(0));
        Ok(Self {
            tasks,
            ready,
            channels: Vec::new(),
            handles: Vec::new(),
            reply_rights: Vec::new(),
            reply_slots: Vec::new(),
            max_tasks,
            max_channels,
            max_replies,
            stats: RuntimeStats {
                tasks_created: 1,
                ..RuntimeStats::default()
            },
        })
    }

    pub fn stats(&self) -> RuntimeStats {
        self.stats
    }

    pub fn open_handles(&self) -> usize {
        let channels = self
            .handles
            .iter()
            .filter(|handle| handle.owner.is_some())
            .count();
        let replies = self
            .reply_rights
            .iter()
            .filter(|right| right.owner != ReplyOwner::Closed)
            .count();
        let receivers = self
            .reply_slots
            .iter()
            .filter(|slot| slot.receiver.is_some())
            .count();
        channels + replies + receivers
    }

    pub fn verify(&self) -> Result<(), String> {
        let mut senders = vec![0usize; self.channels.len()];
        let mut receivers = vec![0usize; self.channels.len()];
        let mut queued_rights = vec![0usize; self.reply_rights.len()];
        for handle in &self.handles {
            if handle.channel >= self.channels.len() {
                return Err("handle has invalid channel".into());
            }
            if let Some(owner) = handle.owner {
                let task = self.tasks.get(owner.0).ok_or("handle has invalid owner")?;
                if matches!(task.state, TaskState::Done(_) | TaskState::Failed(_)) {
                    return Err("terminal task still owns a handle".into());
                }
                match handle.kind {
                    HandleKind::Sender => senders[handle.channel] += 1,
                    HandleKind::Receiver => receivers[handle.channel] += 1,
                }
            }
        }
        for (index, channel) in self.channels.iter().enumerate() {
            if channel.capacity == 0
                || channel.queue.len() > channel.capacity
                || channel.senders != senders[index]
                || channel.receivers != receivers[index]
                || (channel.receivers == 0 && !channel.queue.is_empty())
            {
                return Err(format!("channel {index} violates bounded ownership"));
            }
            for message in &channel.queue {
                match message {
                    Message::Scalar(_) if channel.kind == ChannelKind::Scalar => {}
                    Message::Request(request) if channel.kind == ChannelKind::Request => {
                        let right = self
                            .reply_rights
                            .get(request.reply.token.0)
                            .ok_or("queued request has invalid reply token")?;
                        if right.owner != ReplyOwner::Queue(index) {
                            return Err("queued request does not own its reply right".into());
                        }
                        queued_rights[request.reply.token.0] += 1;
                    }
                    _ => return Err("channel contains wrong message shape".into()),
                }
            }
        }
        for (index, slot) in self.reply_slots.iter().enumerate() {
            let right = self
                .reply_rights
                .get(slot.right.0)
                .ok_or("reply slot has invalid right")?;
            if right.slot != index
                || (slot.value.is_some()
                    && (right.owner != ReplyOwner::Closed || slot.receiver.is_none()))
            {
                return Err("reply slot violates one-shot ownership".into());
            }
            if let Some(owner) = slot.receiver {
                let task = self
                    .tasks
                    .get(owner.0)
                    .ok_or("reply receiver has invalid owner")?;
                if matches!(task.state, TaskState::Done(_) | TaskState::Failed(_)) {
                    return Err("terminal task owns a reply receiver".into());
                }
            }
        }
        for (index, right) in self.reply_rights.iter().enumerate() {
            match right.owner {
                ReplyOwner::Task(owner) => {
                    let task = self
                        .tasks
                        .get(owner.0)
                        .ok_or("reply right has invalid owner")?;
                    if matches!(task.state, TaskState::Done(_) | TaskState::Failed(_)) {
                        return Err("terminal task owns a reply right".into());
                    }
                    if queued_rights[index] != 0 {
                        return Err("task-owned reply right is also queued".into());
                    }
                }
                ReplyOwner::Queue(channel) => {
                    if channel >= self.channels.len() || queued_rights[index] != 1 {
                        return Err("queued reply right lacks one message owner".into());
                    }
                }
                ReplyOwner::Closed if queued_rights[index] != 0 => {
                    return Err("closed reply right remains queued".into());
                }
                ReplyOwner::Closed => {}
            }
        }
        for (index, task) in self.tasks.iter().enumerate() {
            let waiters = match task.state {
                TaskState::Waiting(Wait::Send(channel)) => {
                    self.channels.get(channel).map(|item| &item.send_waiters)
                }
                TaskState::Waiting(Wait::Receive(channel)) => {
                    self.channels.get(channel).map(|item| &item.receive_waiters)
                }
                TaskState::Waiting(Wait::Reply(slot)) => {
                    self.reply_slots.get(slot).map(|item| &item.waiters)
                }
                TaskState::Waiting(Wait::Join) => {
                    if index != 0 {
                        return Err("non-root task waits for scope join".into());
                    }
                    continue;
                }
                TaskState::Waiting(Wait::Yield) => {
                    return Err("yielded task must be runnable".into());
                }
                _ => continue,
            };
            if !waiters.is_some_and(|queue| queue.contains(&TaskId(index))) {
                return Err(format!("task {index} waits without registration"));
            }
        }
        Ok(())
    }

    fn check_handle(&self, task: TaskId, token: Token, kind: HandleKind) -> Result<usize, String> {
        let handle = self
            .handles
            .get(token.0)
            .ok_or("invalid capability token")?;
        if handle.owner != Some(task) || handle.kind != kind {
            return Err("capability is closed or owned by another task".into());
        }
        Ok(handle.channel)
    }

    fn wake_one(&mut self, channel: usize, send: bool) {
        loop {
            let next = if send {
                self.channels[channel].send_waiters.pop_front()
            } else {
                self.channels[channel].receive_waiters.pop_front()
            };
            let Some(task) = next else { break };
            let expected = if send {
                Wait::Send(channel)
            } else {
                Wait::Receive(channel)
            };
            if matches!(self.tasks[task.0].state, TaskState::Waiting(wait) if wait == expected) {
                self.tasks[task.0].state = TaskState::Runnable;
                self.ready.push_back(task);
                self.stats.wakes += 1;
                break;
            }
        }
    }

    fn wake_all(&mut self, channel: usize, send: bool) {
        while if send {
            !self.channels[channel].send_waiters.is_empty()
        } else {
            !self.channels[channel].receive_waiters.is_empty()
        } {
            self.wake_one(channel, send);
        }
    }

    fn wake_reply(&mut self, slot: usize) {
        while let Some(task) = self.reply_slots[slot].waiters.pop_front() {
            if matches!(self.tasks[task.0].state, TaskState::Waiting(Wait::Reply(waiting)) if waiting == slot)
            {
                self.tasks[task.0].state = TaskState::Runnable;
                self.ready.push_back(task);
                self.stats.wakes += 1;
                break;
            }
        }
    }

    fn close_reply_right(&mut self, token: ReplyToken, owner: ReplyOwner) -> Result<(), String> {
        let right = self
            .reply_rights
            .get_mut(token.0)
            .ok_or("invalid reply token")?;
        if right.owner != owner {
            return Err("reply right is closed or owned elsewhere".into());
        }
        right.owner = ReplyOwner::Closed;
        let slot = right.slot;
        self.wake_reply(slot);
        Ok(())
    }

    fn cancel_reply_receiver(&mut self, slot: usize, task: TaskId) -> Result<(), String> {
        let reply = self
            .reply_slots
            .get_mut(slot)
            .ok_or("invalid reply receiver")?;
        if reply.receiver != Some(task) {
            return Err("reply receiver is closed or owned elsewhere".into());
        }
        reply.receiver = None;
        if reply.value.take().is_some() {
            self.stats.discarded_messages += 1;
        }
        Ok(())
    }

    fn wake_joiners(&mut self) {
        if self.tasks.iter().all(|task| {
            matches!(
                task.state,
                TaskState::Done(_) | TaskState::Failed(_) | TaskState::Waiting(Wait::Join)
            )
        }) {
            for index in 0..self.tasks.len() {
                if matches!(self.tasks[index].state, TaskState::Waiting(Wait::Join)) {
                    self.tasks[index].state = TaskState::Runnable;
                    self.ready.push_back(TaskId(index));
                    self.stats.wakes += 1;
                }
            }
        }
    }

    fn close_token(&mut self, task: TaskId, token: Token, kind: HandleKind) -> Result<(), String> {
        let channel = self.check_handle(task, token, kind)?;
        self.handles[token.0].owner = None;
        match kind {
            HandleKind::Sender => {
                self.channels[channel].senders -= 1;
                if self.channels[channel].senders == 0 {
                    self.wake_all(channel, false);
                }
            }
            HandleKind::Receiver => {
                self.channels[channel].receivers -= 1;
                if self.channels[channel].receivers == 0 {
                    while let Some(message) = self.channels[channel].queue.pop_front() {
                        self.stats.discarded_messages += 1;
                        if let Message::Request(request) = message {
                            self.close_reply_right(
                                request.reply.token,
                                ReplyOwner::Queue(channel),
                            )?;
                        }
                    }
                    self.wake_all(channel, true);
                }
            }
        }
        Ok(())
    }

    fn finish_task(&mut self, task: TaskId, outcome: Result<i64, String>) {
        let owned = self
            .handles
            .iter()
            .enumerate()
            .filter_map(|(index, handle)| {
                (handle.owner == Some(task)).then_some((Token(index), handle.kind))
            })
            .collect::<Vec<_>>();
        let owned_rights = self
            .reply_rights
            .iter()
            .enumerate()
            .filter_map(|(index, right)| {
                (right.owner == ReplyOwner::Task(task)).then_some(ReplyToken(index))
            })
            .collect::<Vec<_>>();
        let owned_reply_receivers = self
            .reply_slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| (slot.receiver == Some(task)).then_some(index))
            .collect::<Vec<_>>();
        let outcome = if outcome.is_ok()
            && (!owned.is_empty() || !owned_rights.is_empty() || !owned_reply_receivers.is_empty())
        {
            Err("task exited with open communication capabilities".into())
        } else {
            outcome
        };
        for (token, kind) in owned {
            self.close_token(task, token, kind)
                .expect("owned handle closes once");
        }
        for token in owned_rights {
            self.close_reply_right(token, ReplyOwner::Task(task))
                .expect("owned reply closes once");
        }
        for slot in owned_reply_receivers {
            self.cancel_reply_receiver(slot, task)
                .expect("owned reply receiver closes once");
        }
        self.tasks[task.0].state = match outcome {
            Ok(value) => TaskState::Done(value),
            Err(error) => TaskState::Failed(error),
        };
        self.tasks[task.0].body = None;
        self.wake_joiners();
    }

    pub fn run(&mut self) -> Result<i64, String> {
        while let Some(task) = self.ready.pop_front() {
            if !matches!(self.tasks[task.0].state, TaskState::Runnable) {
                continue;
            }
            let mut body = self.tasks[task.0]
                .body
                .take()
                .expect("runnable task has a frame");
            let (result, wait) = {
                let mut context = Context {
                    scheduler: self,
                    task,
                    wait: None,
                };
                let result = catch_unwind(AssertUnwindSafe(|| body.poll(&mut context)));
                (result, context.wait)
            };
            match result {
                Ok(Ok(Poll::Ready(value))) => self.finish_task(task, Ok(value)),
                Ok(Ok(Poll::Pending)) if wait == Some(Wait::Yield) => {
                    self.tasks[task.0].body = Some(body);
                    self.tasks[task.0].state = TaskState::Runnable;
                    self.ready.push_back(task);
                    self.stats.yields += 1;
                }
                Ok(Ok(Poll::Pending)) if wait.is_some() => {
                    self.tasks[task.0].body = Some(body);
                    self.tasks[task.0].state = TaskState::Waiting(wait.expect("checked"));
                    self.stats.parks += 1;
                }
                Ok(Ok(Poll::Pending)) => {
                    self.finish_task(task, Err("task parked without a wait reason".into()))
                }
                Ok(Err(error)) => self.finish_task(task, Err(error)),
                Err(_) => self.finish_task(task, Err("task panicked".into())),
            }
        }
        if self
            .tasks
            .iter()
            .any(|task| matches!(task.state, TaskState::Runnable | TaskState::Waiting(_)))
        {
            let waiting = (0..self.tasks.len())
                .filter(|index| {
                    matches!(
                        self.tasks[*index].state,
                        TaskState::Runnable | TaskState::Waiting(_)
                    )
                })
                .map(TaskId)
                .collect::<Vec<_>>();
            for task in waiting {
                self.finish_task(task, Err("scope deadlocked".into()));
            }
            return Err("scope deadlocked".into());
        }
        if let Some((index, task)) = self
            .tasks
            .iter()
            .enumerate()
            .find(|(_, task)| matches!(task.state, TaskState::Failed(_)))
        {
            let TaskState::Failed(error) = &task.state else {
                unreachable!()
            };
            return Err(format!("task {index} failed: {error}"));
        }
        let TaskState::Done(value) = self.tasks[0].state else {
            unreachable!("root finished")
        };
        Ok(value)
    }
}

impl Context<'_> {
    pub fn reply(&mut self) -> Result<(ReplyTo, ReplyReceiver), String> {
        self.reply_typed().map_err(|error| error.to_string())
    }

    pub fn reply_typed(&mut self) -> Result<(ReplyTo, ReplyReceiver), ResourceError> {
        if self.scheduler.reply_rights.len() >= self.scheduler.max_replies {
            return Err(ResourceError::ReplyLimit);
        }
        self.scheduler
            .reply_rights
            .try_reserve(1)
            .map_err(|_| ResourceError::Allocation)?;
        self.scheduler
            .reply_slots
            .try_reserve(1)
            .map_err(|_| ResourceError::Allocation)?;
        let slot = self.scheduler.reply_slots.len();
        let token = ReplyToken(self.scheduler.reply_rights.len());
        self.scheduler.reply_rights.push(ReplyRight {
            slot,
            owner: ReplyOwner::Task(self.task),
        });
        self.scheduler.reply_slots.push(ReplySlot {
            right: token,
            receiver: Some(self.task),
            value: None,
            waiters: VecDeque::new(),
        });
        Ok((ReplyTo { token }, ReplyReceiver { slot }))
    }

    pub fn complete(&mut self, right: ReplyTo, value: i64) -> Result<CompleteResult, String> {
        let slot = self
            .scheduler
            .reply_rights
            .get(right.token.0)
            .ok_or("invalid reply token")?;
        if slot.owner != ReplyOwner::Task(self.task) {
            return Err("reply right is closed or owned elsewhere".into());
        }
        let slot = slot.slot;
        let delivered = self.scheduler.reply_slots[slot].receiver.is_some();
        if delivered {
            self.scheduler.reply_slots[slot].value = Some(value);
        }
        self.scheduler
            .close_reply_right(right.token, ReplyOwner::Task(self.task))?;
        Ok(if delivered {
            CompleteResult::Delivered
        } else {
            CompleteResult::Closed(value)
        })
    }

    pub fn close_reply_to(&mut self, right: ReplyTo) -> Result<(), String> {
        self.scheduler
            .close_reply_right(right.token, ReplyOwner::Task(self.task))
    }

    pub fn receive_reply(
        &mut self,
        receiver: &ReplyReceiver,
    ) -> Result<ReplyReceiveResult, String> {
        let slot = self
            .scheduler
            .reply_slots
            .get(receiver.slot)
            .ok_or("invalid reply receiver")?;
        if slot.receiver != Some(self.task) {
            return Err("reply receiver is closed or owned elsewhere".into());
        }
        if let Some(value) = self.scheduler.reply_slots[receiver.slot].value.take() {
            self.scheduler.reply_slots[receiver.slot].receiver = None;
            return Ok(ReplyReceiveResult::Value(value));
        }
        let right = self.scheduler.reply_slots[receiver.slot].right;
        if self.scheduler.reply_rights[right.0].owner == ReplyOwner::Closed {
            self.scheduler.reply_slots[receiver.slot].receiver = None;
            return Ok(ReplyReceiveResult::End);
        }
        if self.wait.is_some() {
            return Err("task already registered a wait in this poll".into());
        }
        self.scheduler.reply_slots[receiver.slot]
            .waiters
            .try_reserve(1)
            .map_err(|error| error.to_string())?;
        self.scheduler.reply_slots[receiver.slot]
            .waiters
            .push_back(self.task);
        self.wait = Some(Wait::Reply(receiver.slot));
        Ok(ReplyReceiveResult::Pending)
    }

    pub fn cancel_reply(&mut self, receiver: ReplyReceiver) -> Result<(), String> {
        self.scheduler
            .cancel_reply_receiver(receiver.slot, self.task)
    }

    pub fn send_request(
        &mut self,
        sender: &Sender,
        request: Request,
    ) -> Result<RequestSendResult, String> {
        self.send_request_owned(sender, request)
            .map_err(|error| error.message)
    }

    /// Send a request while preserving the entire request on a pre-commit error.
    pub fn send_request_owned(
        &mut self,
        sender: &Sender,
        request: Request,
    ) -> Result<RequestSendResult, OwnedError<Request>> {
        let channel = match self
            .scheduler
            .check_handle(self.task, sender.token, HandleKind::Sender)
        {
            Ok(channel) => channel,
            Err(message) => {
                return Err(OwnedError {
                    message,
                    resource_error: None,
                    value: request,
                });
            }
        };
        if self.scheduler.channels[channel].kind != ChannelKind::Request {
            return Err(OwnedError {
                message: "request send requires a request channel".into(),
                resource_error: None,
                value: request,
            });
        }
        let right = match self.scheduler.reply_rights.get(request.reply.token.0) {
            Some(right) => right,
            None => {
                return Err(OwnedError {
                    message: "invalid reply token".into(),
                    resource_error: None,
                    value: request,
                });
            }
        };
        if right.owner != ReplyOwner::Task(self.task) {
            return Err(OwnedError {
                message: "reply right is closed or owned elsewhere".into(),
                resource_error: None,
                value: request,
            });
        }
        if self.scheduler.channels[channel].receivers == 0 {
            return Ok(RequestSendResult::Closed(request));
        }
        if self.scheduler.channels[channel].queue.len() == self.scheduler.channels[channel].capacity
        {
            if self.wait.is_some() {
                return Err(OwnedError {
                    message: "task already registered a wait in this poll".into(),
                    resource_error: None,
                    value: request,
                });
            }
            if let Err(error) = self.scheduler.channels[channel].send_waiters.try_reserve(1) {
                return Err(OwnedError {
                    message: error.to_string(),
                    resource_error: Some(ResourceError::Allocation),
                    value: request,
                });
            }
            self.scheduler.channels[channel]
                .send_waiters
                .push_back(self.task);
            self.wait = Some(Wait::Send(channel));
            return Ok(RequestSendResult::Pending(request));
        }
        self.scheduler.reply_rights[request.reply.token.0].owner = ReplyOwner::Queue(channel);
        self.scheduler.channels[channel]
            .queue
            .push_back(Message::Request(request));
        self.scheduler.stats.max_queued = self
            .scheduler
            .stats
            .max_queued
            .max(self.scheduler.channels[channel].queue.len());
        self.scheduler.wake_one(channel, false);
        Ok(RequestSendResult::Sent)
    }

    pub fn receive_request(&mut self, receiver: &Receiver) -> Result<RequestReceiveResult, String> {
        let channel =
            self.scheduler
                .check_handle(self.task, receiver.token, HandleKind::Receiver)?;
        if self.scheduler.channels[channel].kind != ChannelKind::Request {
            return Err("request receive requires a request channel".into());
        }
        if let Some(message) = self.scheduler.channels[channel].queue.pop_front() {
            let Message::Request(request) = message else {
                unreachable!("typed request channel")
            };
            let right = &mut self.scheduler.reply_rights[request.reply.token.0];
            if right.owner != ReplyOwner::Queue(channel) {
                return Err("queued reply ownership mismatch".into());
            }
            right.owner = ReplyOwner::Task(self.task);
            self.scheduler.wake_one(channel, true);
            return Ok(RequestReceiveResult::Value(request));
        }
        if self.scheduler.channels[channel].senders == 0 {
            return Ok(RequestReceiveResult::End);
        }
        if self.wait.is_some() {
            return Err("task already registered a wait in this poll".into());
        }
        self.scheduler.channels[channel]
            .receive_waiters
            .try_reserve(1)
            .map_err(|error| error.to_string())?;
        self.scheduler.channels[channel]
            .receive_waiters
            .push_back(self.task);
        self.wait = Some(Wait::Receive(channel));
        Ok(RequestReceiveResult::Pending)
    }

    pub fn yield_now(&mut self) -> Result<(), String> {
        if self.wait.is_some() {
            return Err("task already registered a wait in this poll".into());
        }
        self.wait = Some(Wait::Yield);
        Ok(())
    }

    pub fn task_result(&self, task: TaskId) -> Result<Option<i64>, String> {
        match &self
            .scheduler
            .tasks
            .get(task.0)
            .ok_or("unknown task")?
            .state
        {
            TaskState::Done(value) => Ok(Some(*value)),
            TaskState::Failed(error) => Err(error.clone()),
            TaskState::Runnable | TaskState::Waiting(_) => Ok(None),
        }
    }

    pub fn channel(&mut self, capacity: usize) -> Result<(Sender, Receiver), String> {
        self.channel_typed(capacity)
            .map_err(|error| error.to_string())
    }

    pub fn request_channel(&mut self, capacity: usize) -> Result<(Sender, Receiver), String> {
        self.request_channel_typed(capacity)
            .map_err(|error| error.to_string())
    }

    pub fn channel_typed(&mut self, capacity: usize) -> Result<(Sender, Receiver), ResourceError> {
        self.create_channel(capacity, ChannelKind::Scalar)
    }

    pub fn request_channel_typed(
        &mut self,
        capacity: usize,
    ) -> Result<(Sender, Receiver), ResourceError> {
        self.create_channel(capacity, ChannelKind::Request)
    }

    fn create_channel(
        &mut self,
        capacity: usize,
        kind: ChannelKind,
    ) -> Result<(Sender, Receiver), ResourceError> {
        if capacity == 0 {
            return Err(ResourceError::InvalidChannelCapacity);
        }
        if self.scheduler.channels.len() >= self.scheduler.max_channels {
            return Err(ResourceError::ChannelLimit);
        }
        let mut queue = VecDeque::new();
        queue
            .try_reserve_exact(capacity)
            .map_err(|_| ResourceError::Allocation)?;
        self.scheduler
            .channels
            .try_reserve(1)
            .map_err(|_| ResourceError::Allocation)?;
        self.scheduler
            .handles
            .try_reserve(2)
            .map_err(|_| ResourceError::Allocation)?;
        let channel = self.scheduler.channels.len();
        self.scheduler.channels.push(Channel {
            queue,
            kind,
            capacity,
            senders: 1,
            receivers: 1,
            send_waiters: VecDeque::new(),
            receive_waiters: VecDeque::new(),
        });
        let sender = Sender {
            token: Token(self.scheduler.handles.len()),
        };
        self.scheduler.handles.push(Handle {
            channel,
            kind: HandleKind::Sender,
            owner: Some(self.task),
        });
        let receiver = Receiver {
            token: Token(self.scheduler.handles.len()),
        };
        self.scheduler.handles.push(Handle {
            channel,
            kind: HandleKind::Receiver,
            owner: Some(self.task),
        });
        Ok((sender, receiver))
    }

    pub fn fork_sender(&mut self, sender: &Sender) -> Result<Sender, String> {
        let channel = self
            .scheduler
            .check_handle(self.task, sender.token, HandleKind::Sender)?;
        self.scheduler
            .handles
            .try_reserve(1)
            .map_err(|error| error.to_string())?;
        self.scheduler.channels[channel].senders += 1;
        let fork = Sender {
            token: Token(self.scheduler.handles.len()),
        };
        self.scheduler.handles.push(Handle {
            channel,
            kind: HandleKind::Sender,
            owner: Some(self.task),
        });
        Ok(fork)
    }

    pub fn fork_receiver(&mut self, receiver: &Receiver) -> Result<Receiver, String> {
        let channel =
            self.scheduler
                .check_handle(self.task, receiver.token, HandleKind::Receiver)?;
        self.scheduler
            .handles
            .try_reserve(1)
            .map_err(|error| error.to_string())?;
        self.scheduler.channels[channel].receivers += 1;
        let fork = Receiver {
            token: Token(self.scheduler.handles.len()),
        };
        self.scheduler.handles.push(Handle {
            channel,
            kind: HandleKind::Receiver,
            owner: Some(self.task),
        });
        Ok(fork)
    }

    pub fn spawn(
        &mut self,
        child: impl Task + 'static,
        transfers: &[Token],
    ) -> Result<TaskId, String> {
        self.spawn_owned(child, transfers)
            .map_err(|error| error.message)
    }

    /// Spawn a child while preserving its frame on a pre-commit error.
    pub fn spawn_owned<T: Task + 'static>(
        &mut self,
        child: T,
        transfers: &[Token],
    ) -> Result<TaskId, OwnedError<T>> {
        if self.scheduler.tasks.len() >= self.scheduler.max_tasks {
            return Err(OwnedError {
                message: "scope task limit exceeded".into(),
                resource_error: Some(ResourceError::TaskLimit),
                value: child,
            });
        }
        for (index, token) in transfers.iter().enumerate() {
            if transfers[..index].contains(token) {
                return Err(OwnedError {
                    message: "duplicate capability transfer".into(),
                    resource_error: None,
                    value: child,
                });
            }
            let Some(handle) = self.scheduler.handles.get(token.0) else {
                return Err(OwnedError {
                    message: "invalid capability token".into(),
                    resource_error: None,
                    value: child,
                });
            };
            if handle.owner != Some(self.task) {
                return Err(OwnedError {
                    message: "capability is closed or owned by another task".into(),
                    resource_error: None,
                    value: child,
                });
            }
        }
        if let Err(error) = self.scheduler.tasks.try_reserve(1) {
            return Err(OwnedError {
                message: error.to_string(),
                resource_error: Some(ResourceError::Allocation),
                value: child,
            });
        }
        if let Err(error) = self.scheduler.ready.try_reserve(1) {
            return Err(OwnedError {
                message: error.to_string(),
                resource_error: Some(ResourceError::Allocation),
                value: child,
            });
        }
        let id = TaskId(self.scheduler.tasks.len());
        self.scheduler.tasks.push(TaskRecord {
            body: Some(Box::new(child)),
            state: TaskState::Runnable,
        });
        for token in transfers {
            self.scheduler.handles[token.0].owner = Some(id);
        }
        self.scheduler.ready.push_back(id);
        self.scheduler.stats.tasks_created += 1;
        Ok(id)
    }

    pub fn send(&mut self, sender: &Sender, message: i64) -> Result<SendResult, String> {
        let channel = self
            .scheduler
            .check_handle(self.task, sender.token, HandleKind::Sender)?;
        if self.scheduler.channels[channel].kind != ChannelKind::Scalar {
            return Err("scalar send requires an I64 channel".into());
        }
        if self.scheduler.channels[channel].receivers == 0 {
            return Ok(SendResult::Closed(message));
        }
        if self.scheduler.channels[channel].queue.len() == self.scheduler.channels[channel].capacity
        {
            if self.wait.is_some() {
                return Err("task already registered a wait in this poll".into());
            }
            self.scheduler.channels[channel]
                .send_waiters
                .try_reserve(1)
                .map_err(|error| error.to_string())?;
            self.scheduler.channels[channel]
                .send_waiters
                .push_back(self.task);
            self.wait = Some(Wait::Send(channel));
            return Ok(SendResult::Pending(message));
        }
        self.scheduler.channels[channel]
            .queue
            .push_back(Message::Scalar(message));
        self.scheduler.stats.max_queued = self
            .scheduler
            .stats
            .max_queued
            .max(self.scheduler.channels[channel].queue.len());
        self.scheduler.wake_one(channel, false);
        Ok(SendResult::Sent)
    }

    pub fn receive(&mut self, receiver: &Receiver) -> Result<ReceiveResult, String> {
        let channel =
            self.scheduler
                .check_handle(self.task, receiver.token, HandleKind::Receiver)?;
        if self.scheduler.channels[channel].kind != ChannelKind::Scalar {
            return Err("scalar receive requires an I64 channel".into());
        }
        if let Some(message) = self.scheduler.channels[channel].queue.pop_front() {
            let Message::Scalar(value) = message else {
                unreachable!("typed channel queue")
            };
            self.scheduler.wake_one(channel, true);
            return Ok(ReceiveResult::Value(value));
        }
        if self.scheduler.channels[channel].senders == 0 {
            return Ok(ReceiveResult::End);
        }
        if self.wait.is_some() {
            return Err("task already registered a wait in this poll".into());
        }
        self.scheduler.channels[channel]
            .receive_waiters
            .try_reserve(1)
            .map_err(|error| error.to_string())?;
        self.scheduler.channels[channel]
            .receive_waiters
            .push_back(self.task);
        self.wait = Some(Wait::Receive(channel));
        Ok(ReceiveResult::Pending)
    }

    pub fn close_sender(&mut self, sender: Sender) -> Result<(), String> {
        self.scheduler
            .close_token(self.task, sender.token, HandleKind::Sender)
    }

    pub fn close_receiver(&mut self, receiver: Receiver) -> Result<(), String> {
        self.scheduler
            .close_token(self.task, receiver.token, HandleKind::Receiver)
    }

    pub fn join(&mut self) -> Result<Poll<()>, String> {
        if self.task != TaskId(0) {
            return Err("only the scope root may join this runtime slice".into());
        }
        if self
            .scheduler
            .tasks
            .iter()
            .enumerate()
            .all(|(index, task)| {
                index == self.task.0
                    || matches!(task.state, TaskState::Done(_) | TaskState::Failed(_))
            })
        {
            Ok(Poll::Ready(()))
        } else {
            if self.wait.is_some() {
                return Err("task already registered a wait in this poll".into());
            }
            self.wait = Some(Wait::Join);
            Ok(Poll::Pending)
        }
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
