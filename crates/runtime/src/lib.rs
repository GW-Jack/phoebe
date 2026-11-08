//! Scope-owned cooperative task, channel, and one-shot reply kernel.
//! Compiler-generated continuations and typed message layouts are future work.

use std::alloc::{Layout, alloc};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::Poll;

/// Allocate a task body without invoking Rust's infallible `Box::new` path.
/// On allocation failure the caller retains the original, unmodified value.
fn try_box<T>(value: T) -> Result<Box<T>, T> {
    // SAFETY: the closure delegates to the global allocator for this layout.
    unsafe { try_box_with(value, |layout| alloc(layout)) }
}

/// # Safety
///
/// A non-null result from `allocate` must be a fresh allocation from the
/// global allocator for the supplied layout.
unsafe fn try_box_with<T>(value: T, allocate: impl FnOnce(Layout) -> *mut u8) -> Result<Box<T>, T> {
    let layout = Layout::new::<T>();
    if layout.size() == 0 {
        return Ok(Box::new(value));
    }
    // SAFETY: `alloc` returns memory aligned for `T` or null. We write one
    // initialized `T` only after checking the pointer and give its exact
    // allocation to Box, which owns both the value and deallocation.
    let pointer = allocate(layout);
    if pointer.is_null() {
        return Err(value);
    }
    let pointer = pointer.cast::<T>();
    unsafe {
        pointer.write(value);
        Ok(Box::from_raw(pointer))
    }
}

#[cfg(test)]
mod allocation_tests {
    use super::*;

    struct NeverRun {
        sender: Sender,
    }

    impl Task for NeverRun {
        fn poll(&mut self, _context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            panic!("allocation failure must not schedule the child")
        }
    }

    struct FailedSpawnRoot;

    impl Task for FailedSpawnRoot {
        fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
            let (sender, receiver) = context.channel(1)?;
            let token = sender.token();
            let error = context
                .spawn_with_transfers(
                    NeverRun { sender },
                    [CapabilityTransfer::Channel(token)].into_iter(),
                    Err,
                )
                .unwrap_err();
            assert_eq!(error.resource_error, Some(ResourceError::Allocation));
            assert_eq!(error.value.sender.token(), token);
            context.close_sender(error.value.sender)?;
            context.close_receiver(receiver)?;
            Ok(Poll::Ready(42))
        }
    }

    #[test]
    fn failed_frame_allocation_returns_the_original_value() {
        let owned = String::from("retained child");
        let pointer = owned.as_ptr();
        // SAFETY: the injected allocator returns null and never publishes memory.
        let returned = unsafe { try_box_with(owned, |_| std::ptr::null_mut()) }.unwrap_err();
        assert_eq!(returned, "retained child");
        assert_eq!(returned.as_ptr(), pointer);
    }

    #[test]
    fn failed_spawn_frame_allocation_keeps_child_and_transfer_with_parent() {
        let mut scheduler = Scheduler::new(FailedSpawnRoot, 2, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), 42);
        assert_eq!(scheduler.stats().tasks_created, 1);
        scheduler.verify().unwrap();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskId(usize);

mod scopes;
pub use scopes::ScopeId;
mod typed_queue;
use typed_queue::TypedQueue;
pub use typed_queue::TypedRightPlan;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Token(usize);

impl Token {
    /// Encodes a channel token in the native runtime's word ABI.
    pub fn native_word(self) -> Result<i64, String> {
        i64::try_from(self.0).map_err(|_| "channel token exceeds native ABI word".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReplyToken(usize);

pub const REQUEST_I64_MESSAGE_TYPE_ID: u64 = 4;

impl ReplyToken {
    /// Encodes a reply right in a flattened native message component.
    pub fn native_word(self) -> Result<i64, String> {
        i64::try_from(self.0).map_err(|_| "reply token exceeds native ABI word".into())
    }

    /// Decode a flattened token. Use still requires the scheduler's owner
    /// check; decoding grants no capability by itself.
    pub fn from_native_word(word: i64) -> Result<Self, String> {
        usize::try_from(word)
            .map(Self)
            .map_err(|_| "invalid reply token word".into())
    }
}

#[derive(Debug)]
pub struct ReplyTo {
    token: ReplyToken,
}

impl ReplyTo {
    pub fn token(&self) -> ReplyToken {
        self.token
    }

    /// Reconstruct a checked Core view; operations still verify task ownership.
    pub fn from_token(token: ReplyToken) -> Self {
        Self { token }
    }

    pub fn transfer(&self) -> CapabilityTransfer {
        CapabilityTransfer::ReplyRight(self.token)
    }
}

#[derive(Debug)]
pub struct ReplyReceiver {
    slot: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplyReceiverToken(usize);

impl ReplyReceiver {
    pub fn token(&self) -> ReplyReceiverToken {
        ReplyReceiverToken(self.slot)
    }

    /// Reconstruct a checked Core view; operations still verify task ownership.
    pub fn from_token(token: ReplyReceiverToken) -> Self {
        Self { slot: token.0 }
    }

    pub fn transfer(&self) -> CapabilityTransfer {
        CapabilityTransfer::ReplyReceiver(ReplyReceiverToken(self.slot))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityTransfer {
    Channel(Token),
    ReplyRight(ReplyToken),
    ReplyReceiver(ReplyReceiverToken),
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

fn typed_send_error(
    message: impl Into<String>,
    resource_error: Option<ResourceError>,
) -> OwnedError<()> {
    OwnedError {
        message: message.into(),
        resource_error,
        value: (),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceError {
    InvalidChannelCapacity,
    ChannelLimit,
    HandleLimit,
    ReplyLimit,
    TaskLimit,
    Allocation,
}

impl ResourceError {
    pub fn native_word(self) -> i64 {
        match self {
            Self::InvalidChannelCapacity => 1,
            Self::ChannelLimit => 2,
            Self::ReplyLimit => 3,
            Self::Allocation => 4,
            Self::TaskLimit => 5,
            Self::HandleLimit => 6,
        }
    }

    pub fn from_native_word(word: i64) -> Result<Self, String> {
        match word {
            1 => Ok(Self::InvalidChannelCapacity),
            2 => Ok(Self::ChannelLimit),
            3 => Ok(Self::ReplyLimit),
            4 => Ok(Self::Allocation),
            5 => Ok(Self::TaskLimit),
            6 => Ok(Self::HandleLimit),
            _ => Err("invalid resource error word".into()),
        }
    }
}

#[cfg(test)]
mod resource_word_tests {
    use super::ResourceError;

    #[test]
    fn resource_codes_preserve_the_native_abi_and_reject_unknown_words() {
        for (error, code) in [
            (ResourceError::InvalidChannelCapacity, 1),
            (ResourceError::ChannelLimit, 2),
            (ResourceError::ReplyLimit, 3),
            (ResourceError::Allocation, 4),
            (ResourceError::TaskLimit, 5),
            (ResourceError::HandleLimit, 6),
        ] {
            assert_eq!(error.native_word(), code);
            assert_eq!(ResourceError::from_native_word(code), Ok(error));
        }
        assert!(ResourceError::from_native_word(0).is_err());
        assert!(ResourceError::from_native_word(7).is_err());
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidChannelCapacity | Self::ChannelLimit => {
                f.write_str("channel capacity or scope channel limit exceeded")
            }
            Self::HandleLimit => f.write_str("scope handle limit exceeded"),
            Self::ReplyLimit => f.write_str("scope reply limit exceeded"),
            Self::TaskLimit => f.write_str("scope task limit exceeded"),
            Self::Allocation => f.write_str("scope allocation failed"),
        }
    }
}

impl std::error::Error for ResourceError {}

/// A synchronous native call borrows the current task context for its duration.
mod native;
pub use native::*;
mod native_reply;
mod native_scope;
pub use native_reply::*;
pub use native_scope::{phoebe_scope_enter, phoebe_scope_join};
mod native_request;
pub use native_request::*;

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

    /// Reconstruct the typed view used by checked Core; the scheduler still
    /// validates the token's current owner on every operation.
    pub fn from_token(token: Token) -> Self {
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

    /// Reconstruct the typed view used by checked Core; the scheduler still
    /// validates the token's current owner on every operation.
    pub fn from_token(token: Token) -> Self {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypedSendResult {
    Sent,
    Pending,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypedReceiveResult {
    Value,
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
    /// Maximum number of nested task scope slots. The root scope does not
    /// count; closed slots can be reused.
    pub scopes: usize,
    pub channels: usize,
    /// Maximum number of channel handles minted in this scope. Tokens are
    /// never reused, so closed handles still count against this budget.
    pub handles: usize,
    pub replies: usize,
}

impl Default for ScopeLimits {
    fn default() -> Self {
        Self {
            tasks: 64,
            scopes: 64,
            channels: 64,
            handles: 256,
            replies: 64,
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
    Typed,
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
    queue: VecDeque<i64>,
    typed: Option<TypedQueue>,
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
    ScopeJoin(ScopeId),
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
    parent: Option<TaskId>,
    home_scope: ScopeId,
    current_scope: ScopeId,
}

pub struct Scheduler {
    tasks: Vec<TaskRecord>,
    scopes: Vec<scopes::ScopeRecord>,
    free_scope_head: Option<usize>,
    ready: VecDeque<TaskId>,
    channels: Vec<Channel>,
    handles: Vec<Handle>,
    reply_rights: Vec<ReplyRight>,
    reply_slots: Vec<ReplySlot>,
    max_tasks: usize,
    max_scopes: usize,
    max_channels: usize,
    max_handles: usize,
    max_replies: usize,
    stats: RuntimeStats,
    terminal_error: Option<String>,
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
        Self::with_limits(
            root,
            ScopeLimits {
                tasks: max_tasks,
                scopes: ScopeLimits::default().scopes,
                channels: max_channels,
                handles: max_channels
                    .saturating_mul(2)
                    .max(ScopeLimits::default().handles),
                replies: max_replies,
            },
        )
    }

    pub fn with_limits(root: impl Task + 'static, limits: ScopeLimits) -> Result<Self, String> {
        let ScopeLimits {
            tasks: max_tasks,
            scopes: max_scopes,
            channels: max_channels,
            handles: max_handles,
            replies: max_replies,
        } = limits;
        if max_tasks == 0 {
            return Err("scope requires capacity for its root task".into());
        }
        let mut tasks = Vec::new();
        tasks
            .try_reserve_exact(max_tasks)
            .map_err(|error| error.to_string())?;
        let mut ready = VecDeque::new();
        ready
            .try_reserve_exact(max_tasks)
            .map_err(|error| error.to_string())?;
        let root = try_box(root).map_err(|_| "root task frame allocation failed".to_owned())?;
        let mut scopes = Vec::new();
        scopes
            .try_reserve_exact(1)
            .map_err(|error| error.to_string())?;
        scopes.push(scopes::ScopeRecord::root());
        tasks.push(TaskRecord {
            body: Some(root),
            state: TaskState::Runnable,
            parent: None,
            home_scope: ScopeId::ROOT,
            current_scope: ScopeId::ROOT,
        });
        ready.push_back(TaskId(0));
        Ok(Self {
            tasks,
            scopes,
            free_scope_head: None,
            ready,
            channels: Vec::new(),
            handles: Vec::new(),
            reply_rights: Vec::new(),
            reply_slots: Vec::new(),
            max_tasks,
            max_scopes,
            max_channels,
            max_handles,
            max_replies,
            stats: RuntimeStats {
                tasks_created: 1,
                ..RuntimeStats::default()
            },
            terminal_error: None,
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
        self.verify_scopes()?;
        if self.handles.len() > self.max_handles {
            return Err("scope handle budget exceeded".into());
        }
        if self
            .tasks
            .iter()
            .all(|task| matches!(task.state, TaskState::Done(_) | TaskState::Failed(_)))
            && (!self.ready.is_empty()
                || self.channels.iter().any(|channel| {
                    !channel.send_waiters.is_empty() || !channel.receive_waiters.is_empty()
                })
                || self.reply_slots.iter().any(|slot| !slot.waiters.is_empty()))
        {
            return Err("terminal scope retains waiter registrations".into());
        }
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
            let typed_len = channel.typed.as_ref().map_or(0, TypedQueue::len);
            let typed_ok = match (channel.kind, channel.typed.as_ref()) {
                (ChannelKind::Typed, Some(typed)) => {
                    typed.verify()
                        && typed.capacity() == channel.capacity
                        && channel.queue.is_empty()
                }
                (ChannelKind::Scalar, None) => true,
                _ => false,
            };
            if channel.capacity == 0
                || channel.queue.len() > channel.capacity
                || typed_len > channel.capacity
                || !typed_ok
                || channel.senders != senders[index]
                || channel.receivers != receivers[index]
                || (channel.receivers == 0 && (!channel.queue.is_empty() || typed_len != 0))
            {
                return Err(format!("channel {index} violates bounded ownership"));
            }
            if let Some(typed) = &channel.typed {
                typed.visit_queued_rights(|token| {
                    let right = self
                        .reply_rights
                        .get(token.0)
                        .ok_or("queued typed message has invalid reply token")?;
                    if right.owner != ReplyOwner::Queue(index) {
                        return Err("queued typed message does not own its reply right".into());
                    }
                    queued_rights[token.0] += 1;
                    Ok(())
                })?;
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
            if index == 0 {
                if task.parent.is_some() {
                    return Err("root task has a parent".into());
                }
            } else if task.parent.is_none_or(|parent| parent.0 >= index) {
                return Err(format!("task {index} lacks an earlier parent"));
            }
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
                    if !self.has_live_descendants(TaskId(index)) {
                        return Err(format!("task {index} waits for completed descendants"));
                    }
                    continue;
                }
                TaskState::Waiting(Wait::ScopeJoin(scope)) => {
                    let record = self.scope_record(scope)?;
                    if record.owner != TaskId(index)
                        || record.live == 0
                        || task.current_scope != scope
                    {
                        return Err(format!("task {index} waits for an invalid scope"));
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

    fn is_descendant(&self, task: TaskId, ancestor: TaskId) -> bool {
        let mut parent = self.tasks[task.0].parent;
        while let Some(id) = parent {
            if id == ancestor {
                return true;
            }
            parent = self.tasks[id.0].parent;
        }
        false
    }

    fn has_live_descendants(&self, ancestor: TaskId) -> bool {
        self.tasks.iter().enumerate().any(|(index, task)| {
            self.is_descendant(TaskId(index), ancestor)
                && !matches!(task.state, TaskState::Done(_) | TaskState::Failed(_))
        })
    }

    fn wake_joiners(&mut self) {
        for index in 0..self.tasks.len() {
            if matches!(self.tasks[index].state, TaskState::Waiting(Wait::Join))
                && !self.has_live_descendants(TaskId(index))
            {
                self.tasks[index].state = TaskState::Runnable;
                self.ready.push_back(TaskId(index));
                self.stats.wakes += 1;
            }
            if let TaskState::Waiting(Wait::ScopeJoin(scope)) = self.tasks[index].state
                && self
                    .scope_record(scope)
                    .is_ok_and(|record| record.live == 0)
            {
                self.tasks[index].state = TaskState::Runnable;
                self.ready.push_back(TaskId(index));
                self.stats.wakes += 1;
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
                    while self.channels[channel].queue.pop_front().is_some() {
                        self.stats.discarded_messages += 1;
                    }
                    while self.channels[channel]
                        .typed
                        .as_ref()
                        .is_some_and(|typed| typed.len() != 0)
                    {
                        let count = {
                            let typed = self.channels[channel].typed.as_mut().expect("typed queue");
                            let rights = typed.collect_front_rights()?;
                            for token in rights {
                                if self.reply_rights.get(token.0).map(|right| right.owner)
                                    != Some(ReplyOwner::Queue(channel))
                                {
                                    return Err("queued typed reply ownership mismatch".into());
                                }
                            }
                            rights.len()
                        };
                        for index in 0..count {
                            let token = self.channels[channel]
                                .typed
                                .as_ref()
                                .expect("typed queue")
                                .scratch_right(index);
                            self.close_reply_right(token, ReplyOwner::Queue(channel))?;
                        }
                        self.channels[channel]
                            .typed
                            .as_mut()
                            .expect("typed queue")
                            .discard_front();
                        self.stats.discarded_messages += 1;
                    }
                    self.wake_all(channel, true);
                }
            }
        }
        Ok(())
    }

    fn finish_task(&mut self, task: TaskId, outcome: Result<i64, String>) {
        let outcome = if outcome.is_ok()
            && self.tasks[task.0].current_scope != self.tasks[task.0].home_scope
        {
            Err("task exited with an unjoined nested scope".into())
        } else {
            outcome
        };
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
        self.release_task_from_scopes(task);
        self.wake_joiners();
    }

    fn clear_terminal_waiters(&mut self) {
        self.ready.clear();
        for channel in &mut self.channels {
            channel.send_waiters.clear();
            channel.receive_waiters.clear();
        }
        for slot in &mut self.reply_slots {
            slot.waiters.clear();
        }
    }

    fn cancel_nonterminal_descending(&mut self, ids: std::ops::Range<usize>, reason: &str) {
        // Spawn always assigns a larger ID than the parent. Retire younger
        // frames first so an address borrowed from an ancestor remains valid
        // until every descendant frame has been dropped.
        for other in ids.rev() {
            if !matches!(
                self.tasks[other].state,
                TaskState::Done(_) | TaskState::Failed(_)
            ) {
                self.finish_task(TaskId(other), Err(reason.into()));
            }
        }
    }

    pub fn run(&mut self) -> Result<i64, String> {
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
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
            let detached_body = match result {
                Ok(Ok(Poll::Ready(value))) => {
                    self.finish_task(task, Ok(value));
                    Some(body)
                }
                Ok(Ok(Poll::Pending)) if wait == Some(Wait::Yield) => {
                    self.tasks[task.0].body = Some(body);
                    self.tasks[task.0].state = TaskState::Runnable;
                    self.ready.push_back(task);
                    self.stats.yields += 1;
                    None
                }
                Ok(Ok(Poll::Pending)) if wait.is_some() => {
                    self.tasks[task.0].body = Some(body);
                    self.tasks[task.0].state = TaskState::Waiting(wait.expect("checked"));
                    self.stats.parks += 1;
                    None
                }
                Ok(Ok(Poll::Pending)) => {
                    self.finish_task(task, Err("task parked without a wait reason".into()));
                    Some(body)
                }
                Ok(Err(error)) => {
                    self.finish_task(task, Err(error));
                    Some(body)
                }
                Err(_) => {
                    self.finish_task(task, Err("task panicked".into()));
                    Some(body)
                }
            };
            if let TaskState::Failed(error) = &self.tasks[task.0].state {
                let cause = format!("task {} failed: {error}", task.0);
                self.cancel_nonterminal_descending(task.0 + 1..self.tasks.len(), "scope cancelled");
                drop(detached_body);
                self.cancel_nonterminal_descending(0..task.0, "scope cancelled");
                self.clear_terminal_waiters();
                self.close_terminal_scopes();
                self.terminal_error = Some(cause.clone());
                return Err(cause);
            }
        }
        if self
            .tasks
            .iter()
            .any(|task| matches!(task.state, TaskState::Runnable | TaskState::Waiting(_)))
        {
            self.cancel_nonterminal_descending(0..self.tasks.len(), "scope deadlocked");
            self.clear_terminal_waiters();
            self.close_terminal_scopes();
            self.terminal_error = Some("scope deadlocked".into());
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
        let typed_request = self.scheduler.channels[channel]
            .typed
            .as_ref()
            .is_some_and(TypedQueue::matches_request_i64);
        if !typed_request {
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
        let reply_word = match u64::try_from(request.reply.token.0) {
            Ok(word) => word,
            Err(_) => {
                return Err(OwnedError {
                    message: "reply token exceeds typed message word".into(),
                    resource_error: None,
                    value: request,
                });
            }
        };
        let words = [request.value as u64, reply_word];
        match self.send_typed_words_owned(sender, REQUEST_I64_MESSAGE_TYPE_ID, &words) {
            Ok(TypedSendResult::Sent) => Ok(RequestSendResult::Sent),
            Ok(TypedSendResult::Pending) => Ok(RequestSendResult::Pending(request)),
            Ok(TypedSendResult::Closed) => Ok(RequestSendResult::Closed(request)),
            Err(error) => Err(OwnedError {
                message: error.message,
                resource_error: error.resource_error,
                value: request,
            }),
        }
    }

    pub fn receive_request(&mut self, receiver: &Receiver) -> Result<RequestReceiveResult, String> {
        let channel =
            self.scheduler
                .check_handle(self.task, receiver.token, HandleKind::Receiver)?;
        if !self.scheduler.channels[channel]
            .typed
            .as_ref()
            .is_some_and(TypedQueue::matches_request_i64)
        {
            return Err("request receive requires a request channel".into());
        }
        let mut words = [0; 2];
        match self.receive_typed_words(receiver, REQUEST_I64_MESSAGE_TYPE_ID, &mut words)? {
            TypedReceiveResult::Value => {
                let token = usize::try_from(words[1])
                    .map_err(|_| "queued request reply token exceeds host width")?;
                Ok(RequestReceiveResult::Value(Request {
                    value: words[0] as i64,
                    reply: ReplyTo {
                        token: ReplyToken(token),
                    },
                }))
            }
            TypedReceiveResult::Pending => Ok(RequestReceiveResult::Pending),
            TypedReceiveResult::End => Ok(RequestReceiveResult::End),
        }
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
        self.create_channel(capacity, ChannelKind::Scalar, None)
    }

    pub fn request_channel_typed(
        &mut self,
        capacity: usize,
    ) -> Result<(Sender, Receiver), ResourceError> {
        self.typed_right_channel(
            capacity,
            REQUEST_I64_MESSAGE_TYPE_ID,
            2,
            TypedRightPlan::ReplyTo { part: 1 },
        )
    }

    /// Create a channel for a concrete, capability-free flattened type. The
    /// type identity is checked on every operation, independently of width.
    /// Capability-bearing layouts use the existing concrete request channel
    /// until typed right transfer and discard are implemented.
    pub fn typed_words_channel(
        &mut self,
        capacity: usize,
        type_id: u64,
        width: usize,
    ) -> Result<(Sender, Receiver), ResourceError> {
        self.create_channel(
            capacity,
            ChannelKind::Typed,
            Some((type_id, width, TypedRightPlan::Empty)),
        )
    }

    /// Create a fixed-width channel whose active payload may own reply
    /// rights. The plan is stored once per channel, never per message.
    pub fn typed_right_channel(
        &mut self,
        capacity: usize,
        type_id: u64,
        width: usize,
        plan: TypedRightPlan,
    ) -> Result<(Sender, Receiver), ResourceError> {
        self.create_channel(capacity, ChannelKind::Typed, Some((type_id, width, plan)))
    }

    fn create_channel(
        &mut self,
        capacity: usize,
        kind: ChannelKind,
        typed_layout: Option<(u64, usize, TypedRightPlan)>,
    ) -> Result<(Sender, Receiver), ResourceError> {
        if capacity == 0 {
            return Err(ResourceError::InvalidChannelCapacity);
        }
        if self.scheduler.channels.len() >= self.scheduler.max_channels {
            return Err(ResourceError::ChannelLimit);
        }
        if self
            .scheduler
            .handles
            .len()
            .checked_add(2)
            .is_none_or(|next| next > self.scheduler.max_handles)
        {
            return Err(ResourceError::HandleLimit);
        }
        let typed = match typed_layout {
            Some((type_id, width, plan)) if kind == ChannelKind::Typed => {
                Some(if plan == TypedRightPlan::Empty {
                    TypedQueue::new(type_id, width, capacity)?
                } else {
                    TypedQueue::with_plan(type_id, width, capacity, plan)?
                })
            }
            None if kind == ChannelKind::Scalar => None,
            _ => return Err(ResourceError::InvalidChannelCapacity),
        };
        let mut queue = VecDeque::new();
        if typed.is_none() {
            queue
                .try_reserve_exact(capacity)
                .map_err(|_| ResourceError::Allocation)?;
        }
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
            typed,
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
        self.fork_sender_typed(sender)?
            .map_err(|error| error.to_string())
    }

    pub fn fork_sender_typed(
        &mut self,
        sender: &Sender,
    ) -> Result<Result<Sender, ResourceError>, String> {
        let channel = self
            .scheduler
            .check_handle(self.task, sender.token, HandleKind::Sender)?;
        if self.scheduler.handles.len() >= self.scheduler.max_handles {
            return Ok(Err(ResourceError::HandleLimit));
        }
        if self.scheduler.handles.try_reserve(1).is_err() {
            return Ok(Err(ResourceError::Allocation));
        }
        self.scheduler.channels[channel].senders += 1;
        let fork = Sender {
            token: Token(self.scheduler.handles.len()),
        };
        self.scheduler.handles.push(Handle {
            channel,
            kind: HandleKind::Sender,
            owner: Some(self.task),
        });
        Ok(Ok(fork))
    }

    pub fn fork_receiver(&mut self, receiver: &Receiver) -> Result<Receiver, String> {
        self.fork_receiver_typed(receiver)?
            .map_err(|error| error.to_string())
    }

    pub fn fork_receiver_typed(
        &mut self,
        receiver: &Receiver,
    ) -> Result<Result<Receiver, ResourceError>, String> {
        let channel =
            self.scheduler
                .check_handle(self.task, receiver.token, HandleKind::Receiver)?;
        if self.scheduler.handles.len() >= self.scheduler.max_handles {
            return Ok(Err(ResourceError::HandleLimit));
        }
        if self.scheduler.handles.try_reserve(1).is_err() {
            return Ok(Err(ResourceError::Allocation));
        }
        self.scheduler.channels[channel].receivers += 1;
        let fork = Receiver {
            token: Token(self.scheduler.handles.len()),
        };
        self.scheduler.handles.push(Handle {
            channel,
            kind: HandleKind::Receiver,
            owner: Some(self.task),
        });
        Ok(Ok(fork))
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
        self.spawn_with_transfers(
            child,
            transfers.iter().copied().map(CapabilityTransfer::Channel),
            try_box,
        )
    }

    /// Atomically transfer checked channel and reply capabilities to a child.
    /// The original frame and all rights stay with the caller on an error.
    pub fn spawn_capabilities_owned<T: Task + 'static>(
        &mut self,
        child: T,
        transfers: &[CapabilityTransfer],
    ) -> Result<TaskId, OwnedError<T>> {
        self.spawn_with_transfers(child, transfers.iter().copied(), try_box)
    }

    fn spawn_with_transfers<T: Task + 'static>(
        &mut self,
        child: T,
        transfers: impl Iterator<Item = CapabilityTransfer> + Clone,
        box_frame: impl FnOnce(T) -> Result<Box<T>, T>,
    ) -> Result<TaskId, OwnedError<T>> {
        if self.scheduler.tasks.len() >= self.scheduler.max_tasks {
            return Err(OwnedError {
                message: "scope task limit exceeded".into(),
                resource_error: Some(ResourceError::TaskLimit),
                value: child,
            });
        }
        for (index, transfer) in transfers.clone().enumerate() {
            if transfers.clone().take(index).any(|prior| prior == transfer) {
                return Err(OwnedError {
                    message: "duplicate capability transfer".into(),
                    resource_error: None,
                    value: child,
                });
            }
            let valid = match transfer {
                CapabilityTransfer::Channel(token) => self
                    .scheduler
                    .handles
                    .get(token.0)
                    .is_some_and(|handle| handle.owner == Some(self.task)),
                CapabilityTransfer::ReplyRight(token) => self
                    .scheduler
                    .reply_rights
                    .get(token.0)
                    .is_some_and(|right| right.owner == ReplyOwner::Task(self.task)),
                CapabilityTransfer::ReplyReceiver(token) => self
                    .scheduler
                    .reply_slots
                    .get(token.0)
                    .is_some_and(|reply| reply.receiver == Some(self.task)),
            };
            if !valid {
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
        let child = match box_frame(child) {
            Ok(boxed) => boxed,
            Err(child) => {
                return Err(OwnedError {
                    message: "child task frame allocation failed".into(),
                    resource_error: Some(ResourceError::Allocation),
                    value: child,
                });
            }
        };
        let id = TaskId(self.scheduler.tasks.len());
        self.scheduler.tasks.push(TaskRecord {
            body: Some(child),
            state: TaskState::Runnable,
            parent: Some(self.task),
            home_scope: self.scheduler.tasks[self.task.0].current_scope,
            current_scope: self.scheduler.tasks[self.task.0].current_scope,
        });
        self.scheduler.add_task_to_scopes(id);
        for transfer in transfers {
            match transfer {
                CapabilityTransfer::Channel(token) => {
                    self.scheduler.handles[token.0].owner = Some(id);
                }
                CapabilityTransfer::ReplyRight(token) => {
                    self.scheduler.reply_rights[token.0].owner = ReplyOwner::Task(id);
                }
                CapabilityTransfer::ReplyReceiver(token) => {
                    self.scheduler.reply_slots[token.0].receiver = Some(id);
                }
            }
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
        self.scheduler.channels[channel].queue.push_back(message);
        self.scheduler.stats.max_queued = self
            .scheduler
            .stats
            .max_queued
            .max(self.scheduler.channels[channel].queue.len());
        self.scheduler.wake_one(channel, false);
        Ok(SendResult::Sent)
    }

    /// Send one concrete message. `words` and its active reply rights remain
    /// with the caller on Pending, Closed, and every precommit error. An
    /// available slot is filled without allocating.
    pub fn send_typed_words(
        &mut self,
        sender: &Sender,
        type_id: u64,
        words: &[u64],
    ) -> Result<TypedSendResult, String> {
        self.send_typed_words_owned(sender, type_id, words)
            .map_err(|error| error.message)
    }

    fn send_typed_words_owned(
        &mut self,
        sender: &Sender,
        type_id: u64,
        words: &[u64],
    ) -> Result<TypedSendResult, OwnedError<()>> {
        let channel = self
            .scheduler
            .check_handle(self.task, sender.token, HandleKind::Sender)
            .map_err(|message| typed_send_error(message, None))?;
        let typed = self.scheduler.channels[channel]
            .typed
            .as_ref()
            .ok_or_else(|| typed_send_error("typed send requires a typed channel", None))?;
        if !typed.matches(type_id, words.len()) {
            return Err(typed_send_error(
                "typed message identity or width mismatch",
                None,
            ));
        }
        if self.scheduler.channels[channel].receivers == 0 {
            return Ok(TypedSendResult::Closed);
        }
        if typed.is_full() {
            if self.wait.is_some() {
                return Err(typed_send_error(
                    "task already registered a wait in this poll",
                    None,
                ));
            }
            self.scheduler.channels[channel]
                .send_waiters
                .try_reserve(1)
                .map_err(|error| {
                    typed_send_error(error.to_string(), Some(ResourceError::Allocation))
                })?;
            self.scheduler.channels[channel]
                .send_waiters
                .push_back(self.task);
            self.wait = Some(Wait::Send(channel));
            return Ok(TypedSendResult::Pending);
        }
        let right_count = {
            let typed = self.scheduler.channels[channel]
                .typed
                .as_mut()
                .expect("checked typed channel");
            let rights = typed
                .collect_send_rights(words)
                .map_err(|message| typed_send_error(message, None))?;
            for token in rights {
                if self
                    .scheduler
                    .reply_rights
                    .get(token.0)
                    .map(|right| right.owner)
                    != Some(ReplyOwner::Task(self.task))
                {
                    return Err(typed_send_error(
                        "typed reply right is closed or owned elsewhere",
                        None,
                    ));
                }
            }
            rights.len()
        };
        for index in 0..right_count {
            let token = self.scheduler.channels[channel]
                .typed
                .as_ref()
                .expect("typed queue")
                .scratch_right(index);
            self.scheduler.reply_rights[token.0].owner = ReplyOwner::Queue(channel);
        }
        let typed = self.scheduler.channels[channel]
            .typed
            .as_mut()
            .expect("checked typed channel");
        typed.push(words);
        self.scheduler.stats.max_queued = self.scheduler.stats.max_queued.max(typed.len());
        self.scheduler.wake_one(channel, false);
        Ok(TypedSendResult::Sent)
    }

    pub fn receive_typed_words(
        &mut self,
        receiver: &Receiver,
        type_id: u64,
        output: &mut [u64],
    ) -> Result<TypedReceiveResult, String> {
        let channel =
            self.scheduler
                .check_handle(self.task, receiver.token, HandleKind::Receiver)?;
        let typed = self.scheduler.channels[channel]
            .typed
            .as_mut()
            .ok_or("typed receive requires a typed channel")?;
        if !typed.matches(type_id, output.len()) {
            return Err("typed message identity or width mismatch".into());
        }
        if typed.len() != 0 {
            let right_count = {
                let rights = typed.collect_front_rights()?;
                for token in rights {
                    if self
                        .scheduler
                        .reply_rights
                        .get(token.0)
                        .map(|right| right.owner)
                        != Some(ReplyOwner::Queue(channel))
                    {
                        return Err("queued typed reply ownership mismatch".into());
                    }
                }
                rights.len()
            };
            for index in 0..right_count {
                let token = self.scheduler.channels[channel]
                    .typed
                    .as_ref()
                    .expect("typed queue")
                    .scratch_right(index);
                self.scheduler.reply_rights[token.0].owner = ReplyOwner::Task(self.task);
            }
            self.scheduler.channels[channel]
                .typed
                .as_mut()
                .expect("typed queue")
                .pop(output);
            self.scheduler.wake_one(channel, true);
            return Ok(TypedReceiveResult::Value);
        }
        if self.scheduler.channels[channel].senders == 0 {
            return Ok(TypedReceiveResult::End);
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
        Ok(TypedReceiveResult::Pending)
    }

    pub fn receive(&mut self, receiver: &Receiver) -> Result<ReceiveResult, String> {
        let channel =
            self.scheduler
                .check_handle(self.task, receiver.token, HandleKind::Receiver)?;
        if self.scheduler.channels[channel].kind != ChannelKind::Scalar {
            return Err("scalar receive requires an I64 channel".into());
        }
        if let Some(value) = self.scheduler.channels[channel].queue.pop_front() {
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
        if self.wait.is_some() {
            return Err("task already registered a wait in this poll".into());
        }
        if !self.scheduler.has_live_descendants(self.task) {
            Ok(Poll::Ready(()))
        } else {
            self.wait = Some(Wait::Join);
            Ok(Poll::Pending)
        }
    }
}
