//! Scope-owned cooperative task and bounded I64 channel kernel.
//! Compiler-generated continuations and typed message layouts are future work.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::Poll;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskId(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Token(usize);

#[derive(Debug)]
pub struct Sender {
    token: Token,
}

impl Sender {
    pub fn token(&self) -> Token {
        self.token
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

pub trait Task {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandleKind {
    Sender,
    Receiver,
}

#[derive(Clone, Copy, Debug)]
struct Handle {
    channel: usize,
    kind: HandleKind,
    owner: Option<TaskId>,
}

struct Channel {
    queue: VecDeque<i64>,
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
    max_tasks: usize,
    max_channels: usize,
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
            max_tasks,
            max_channels,
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
        self.handles
            .iter()
            .filter(|handle| handle.owner.is_some())
            .count()
    }

    pub fn verify(&self) -> Result<(), String> {
        let mut senders = vec![0usize; self.channels.len()];
        let mut receivers = vec![0usize; self.channels.len()];
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
        }
        for (index, task) in self.tasks.iter().enumerate() {
            let waiters = match task.state {
                TaskState::Waiting(Wait::Send(channel)) => {
                    self.channels.get(channel).map(|item| &item.send_waiters)
                }
                TaskState::Waiting(Wait::Receive(channel)) => {
                    self.channels.get(channel).map(|item| &item.receive_waiters)
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
                    self.stats.discarded_messages += self.channels[channel].queue.len();
                    self.channels[channel].queue.clear();
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
        let outcome = if outcome.is_ok() && !owned.is_empty() {
            Err("task exited with open communication capabilities".into())
        } else {
            outcome
        };
        for (token, kind) in owned {
            self.close_token(task, token, kind)
                .expect("owned handle closes once");
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
        if capacity == 0 || self.scheduler.channels.len() >= self.scheduler.max_channels {
            return Err("channel capacity or scope channel limit exceeded".into());
        }
        let mut queue = VecDeque::new();
        queue
            .try_reserve_exact(capacity)
            .map_err(|error| error.to_string())?;
        self.scheduler
            .channels
            .try_reserve(1)
            .map_err(|error| error.to_string())?;
        self.scheduler
            .handles
            .try_reserve(2)
            .map_err(|error| error.to_string())?;
        let channel = self.scheduler.channels.len();
        self.scheduler.channels.push(Channel {
            queue,
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
        if self.scheduler.tasks.len() >= self.scheduler.max_tasks {
            return Err("scope task limit exceeded".into());
        }
        for (index, token) in transfers.iter().enumerate() {
            if transfers[..index].contains(token) {
                return Err("duplicate capability transfer".into());
            }
            let handle = self
                .scheduler
                .handles
                .get(token.0)
                .ok_or("invalid capability token")?;
            if handle.owner != Some(self.task) {
                return Err("capability is closed or owned by another task".into());
            }
        }
        self.scheduler
            .tasks
            .try_reserve(1)
            .map_err(|error| error.to_string())?;
        self.scheduler
            .ready
            .try_reserve(1)
            .map_err(|error| error.to_string())?;
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

    pub fn receive(&mut self, receiver: &Receiver) -> Result<ReceiveResult, String> {
        let channel =
            self.scheduler
                .check_handle(self.task, receiver.token, HandleKind::Receiver)?;
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
