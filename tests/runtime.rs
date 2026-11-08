use std::task::Poll;
use std::{cell::RefCell, rc::Rc};

use phoebe::runtime::{
    Context, ReceiveResult, Receiver, Scheduler, ScopeId, SendResult, Sender, Task, TaskId,
};

struct LoggedFrame {
    depth: usize,
    fail_depth: usize,
    ready_with_open_handle: bool,
    polls: usize,
    dropped: Rc<RefCell<Vec<usize>>>,
}

impl Drop for LoggedFrame {
    fn drop(&mut self) {
        self.dropped.borrow_mut().push(self.depth);
    }
}

impl Task for LoggedFrame {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.polls == 0 && self.depth < 2 {
            context.spawn(
                LoggedFrame {
                    depth: self.depth + 1,
                    fail_depth: self.fail_depth,
                    ready_with_open_handle: self.ready_with_open_handle,
                    polls: 0,
                    dropped: self.dropped.clone(),
                },
                &[],
            )?;
        }
        self.polls += 1;
        if self.polls > 1 && self.depth == self.fail_depth {
            if self.ready_with_open_handle {
                let _ = context.channel(1)?;
                return Ok(Poll::Ready(0));
            }
            return Err("deliberate failure".into());
        }
        context.yield_now()?;
        Ok(Poll::Pending)
    }
}

#[test]
fn failure_drops_descendant_frames_before_ancestors() {
    for (fail_depth, ready_with_open_handle) in [(0, false), (1, false), (1, true)] {
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let root = LoggedFrame {
            depth: 0,
            fail_depth,
            ready_with_open_handle,
            polls: 0,
            dropped: dropped.clone(),
        };
        let mut scheduler = Scheduler::new(root, 3, 1, 0).unwrap();
        let error = scheduler.run().unwrap_err();
        assert!(
            error.contains(if ready_with_open_handle {
                "open communication capabilities"
            } else {
                "deliberate failure"
            }),
            "{error}"
        );
        assert_eq!(&*dropped.borrow(), &[2, 1, 0]);
        scheduler.verify().unwrap();
    }
}

struct ParkedFrame {
    depth: usize,
    dropped: Rc<RefCell<Vec<usize>>>,
    sender: Option<Sender>,
    receiver: Option<Receiver>,
}

impl Drop for ParkedFrame {
    fn drop(&mut self) {
        self.dropped.borrow_mut().push(self.depth);
    }
}

impl Task for ParkedFrame {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.receiver.is_none() {
            if self.depth < 2 {
                context.spawn(
                    ParkedFrame {
                        depth: self.depth + 1,
                        dropped: self.dropped.clone(),
                        sender: None,
                        receiver: None,
                    },
                    &[],
                )?;
            }
            let (sender, receiver) = context.channel(1)?;
            self.sender = Some(sender);
            self.receiver = Some(receiver);
        }
        assert_eq!(
            context.receive(self.receiver.as_ref().unwrap())?,
            ReceiveResult::Pending
        );
        Ok(Poll::Pending)
    }
}

#[test]
fn deadlock_drops_descendant_frames_before_ancestors() {
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let root = ParkedFrame {
        depth: 0,
        dropped: dropped.clone(),
        sender: None,
        receiver: None,
    };
    let mut scheduler = Scheduler::new(root, 3, 3, 0).unwrap();
    assert_eq!(scheduler.run().unwrap_err(), "scope deadlocked");
    assert_eq!(&*dropped.borrow(), &[2, 1, 0]);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct Producer {
    sender: Option<Sender>,
    value: i64,
}

impl Task for Producer {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        match context.send(self.sender.as_ref().expect("sender retained"), self.value)? {
            SendResult::Sent => {
                context.close_sender(self.sender.take().expect("sender retained"))?;
                Ok(Poll::Ready(0))
            }
            SendResult::Pending(value) => {
                assert_eq!(value, self.value);
                Ok(Poll::Pending)
            }
            SendResult::Closed(value) => Err(format!("receiver closed before {value}")),
        }
    }
}

#[derive(Default)]
struct ReceiveRoot {
    receiver: Option<Receiver>,
    value: Option<i64>,
}

impl Task for ReceiveRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.receiver.is_none() && self.value.is_none() {
            let (sender, receiver) = context.channel(1)?;
            let fork = context.fork_sender(&sender)?;
            let token = fork.token();
            context.spawn(
                Producer {
                    sender: Some(fork),
                    value: 41,
                },
                &[token],
            )?;
            context.close_sender(sender)?;
            self.receiver = Some(receiver);
        }
        if let Some(receiver) = &self.receiver {
            match context.receive(receiver)? {
                ReceiveResult::Value(value) => {
                    self.value = Some(value);
                    context.close_receiver(self.receiver.take().expect("receiver retained"))?;
                }
                ReceiveResult::Pending => return Ok(Poll::Pending),
                ReceiveResult::End => return Err("missing producer value".into()),
            }
        }
        match context.join()? {
            Poll::Ready(()) => Ok(Poll::Ready(self.value.expect("received") + 1)),
            Poll::Pending => Ok(Poll::Pending),
        }
    }
}

#[test]
fn scope_spawn_transfer_and_receive_park_without_os_threads() {
    let mut scheduler = Scheduler::new(ReceiveRoot::default(), 2, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    let stats = scheduler.stats();
    assert_eq!(stats.tasks_created, 2);
    assert_eq!(stats.parks, 1);
    assert_eq!(stats.wakes, 1);
    assert_eq!(stats.max_queued, 1);
    scheduler.verify().unwrap();
}

struct Drain {
    receiver: Option<Receiver>,
    sum: i64,
}

impl Task for Drain {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        loop {
            match context.receive(self.receiver.as_ref().expect("receiver retained"))? {
                ReceiveResult::Value(value) => self.sum += value,
                ReceiveResult::Pending => return Ok(Poll::Pending),
                ReceiveResult::End => {
                    context.close_receiver(self.receiver.take().expect("receiver retained"))?;
                    return Ok(Poll::Ready(self.sum));
                }
            }
        }
    }
}

#[derive(Default)]
struct BackpressureRoot {
    sender: Option<Sender>,
    child: Option<TaskId>,
    phase: u8,
}

impl Task for BackpressureRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.phase == 0 {
            let (sender, receiver) = context.channel(1)?;
            let token = receiver.token();
            self.child = Some(context.spawn(
                Drain {
                    receiver: Some(receiver),
                    sum: 0,
                },
                &[token],
            )?);
            self.sender = Some(sender);
            self.phase = 1;
        }
        if self.phase == 1 {
            assert_eq!(
                context.send(self.sender.as_ref().expect("sender"), 20)?,
                SendResult::Sent
            );
            self.phase = 2;
        }
        if self.phase == 2 {
            match context.send(self.sender.as_ref().expect("sender"), 22)? {
                SendResult::Sent => {
                    context.close_sender(self.sender.take().expect("sender"))?;
                    self.phase = 3;
                }
                SendResult::Pending(value) => {
                    assert_eq!(value, 22);
                    return Ok(Poll::Pending);
                }
                SendResult::Closed(_) => return Err("receiver closed early".into()),
            }
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(
                context
                    .task_result(self.child.expect("child"))?
                    .expect("done"),
            )),
        }
    }
}

#[test]
fn one_slot_fifo_backpressure_parks_sender_and_wakes_on_receive() {
    let mut scheduler = Scheduler::new(BackpressureRoot::default(), 2, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    let stats = scheduler.stats();
    assert_eq!(stats.max_queued, 1);
    assert!(stats.parks >= 2);
    assert!(stats.wakes >= 2);
    scheduler.verify().unwrap();
}

struct ClosedRoot;

impl Task for ClosedRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver) = context.channel(1)?;
        assert_eq!(context.send(&sender, 41)?, SendResult::Sent);
        context.close_receiver(receiver)?;
        assert_eq!(context.send(&sender, 42)?, SendResult::Closed(42));
        context.close_sender(sender)?;
        Ok(Poll::Ready(42))
    }
}

#[test]
fn last_receiver_close_discards_queue_and_rejected_send_keeps_message() {
    let mut scheduler = Scheduler::new(ClosedRoot, 1, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.stats().discarded_messages, 1);
    scheduler.verify().unwrap();
}

struct FailingSender {
    _sender: Sender,
}

impl Task for FailingSender {
    fn poll(&mut self, _context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        Err("boom".into())
    }
}

#[derive(Default)]
struct FailureRoot {
    receiver: Option<Receiver>,
}

impl Task for FailureRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.receiver.is_none() {
            let (sender, receiver) = context.channel(1)?;
            let fork = context.fork_sender(&sender)?;
            let token = fork.token();
            context.spawn(FailingSender { _sender: fork }, &[token])?;
            context.close_sender(sender)?;
            self.receiver = Some(receiver);
        }
        match context.receive(self.receiver.as_ref().expect("receiver"))? {
            ReceiveResult::Pending => Ok(Poll::Pending),
            ReceiveResult::End => {
                context.close_receiver(self.receiver.take().expect("receiver"))?;
                match context.join()? {
                    Poll::Pending => Ok(Poll::Pending),
                    Poll::Ready(()) => Ok(Poll::Ready(0)),
                }
            }
            ReceiveResult::Value(_) => Err("unexpected value".into()),
        }
    }
}

#[test]
fn child_failure_closes_owned_sender_and_fails_scope() {
    let mut scheduler = Scheduler::new(FailureRoot::default(), 2, 1, 0).unwrap();
    assert!(scheduler.run().unwrap_err().contains("task 1 failed: boom"));
    assert!(scheduler.stats().wakes >= 1);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

#[derive(Default)]
struct RetainedSenderFailureRoot {
    sender: Option<Sender>,
    receiver: Option<Receiver>,
}

impl Task for RetainedSenderFailureRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.receiver.is_none() {
            let (sender, receiver) = context.channel(1)?;
            let fork = context.fork_sender(&sender)?;
            let token = fork.token();
            context.spawn(FailingSender { _sender: fork }, &[token])?;
            self.sender = Some(sender);
            self.receiver = Some(receiver);
        }
        assert_eq!(
            context.receive(self.receiver.as_ref().expect("receiver"))?,
            ReceiveResult::Pending
        );
        Ok(Poll::Pending)
    }
}

#[test]
fn child_failure_cancels_scope_before_a_retained_sender_masks_it_as_deadlock() {
    let mut scheduler = Scheduler::new(RetainedSenderFailureRoot::default(), 2, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap_err(), "task 1 failed: boom");
    assert_eq!(scheduler.run().unwrap_err(), "task 1 failed: boom");
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct UnpolledReceiver {
    _receiver: Receiver,
}

impl Task for UnpolledReceiver {
    fn poll(&mut self, _context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        Err("cancelled child must not run".into())
    }
}

struct RootFailureWithQueuedMessage;

impl Task for RootFailureWithQueuedMessage {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver) = context.channel(1)?;
        let token = receiver.token();
        context.spawn(
            UnpolledReceiver {
                _receiver: receiver,
            },
            &[token],
        )?;
        assert_eq!(context.send(&sender, 42)?, SendResult::Sent);
        Err("root boom".into())
    }
}

#[test]
fn root_failure_cancels_unpolled_child_and_discards_its_queued_message() {
    let mut scheduler = Scheduler::new(RootFailureWithQueuedMessage, 2, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap_err(), "task 0 failed: root boom");
    assert_eq!(scheduler.stats().discarded_messages, 1);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

#[derive(Default)]
struct DeadlockRoot {
    sender: Option<Sender>,
    receiver: Option<Receiver>,
}

impl Task for DeadlockRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.receiver.is_none() {
            let (sender, receiver) = context.channel(1)?;
            self.sender = Some(sender);
            self.receiver = Some(receiver);
        }
        assert_eq!(
            context.receive(self.receiver.as_ref().expect("receiver"))?,
            ReceiveResult::Pending
        );
        Ok(Poll::Pending)
    }
}

#[test]
fn deadlocked_scope_fails_and_closes_every_handle() {
    let mut scheduler = Scheduler::new(DeadlockRoot::default(), 1, 1, 0).unwrap();
    assert!(scheduler.run().unwrap_err().contains("scope deadlocked"));
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct CloseReceiver {
    receiver: Option<Receiver>,
}

impl Task for CloseReceiver {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        context.close_receiver(self.receiver.take().expect("receiver"))?;
        Ok(Poll::Ready(0))
    }
}

#[derive(Default)]
struct RejectedBlockedSendRoot {
    sender: Option<Sender>,
    phase: u8,
}

impl Task for RejectedBlockedSendRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.phase == 0 {
            let (sender, receiver) = context.channel(1)?;
            let token = receiver.token();
            context.spawn(
                CloseReceiver {
                    receiver: Some(receiver),
                },
                &[token],
            )?;
            assert_eq!(context.send(&sender, 41)?, SendResult::Sent);
            self.sender = Some(sender);
            self.phase = 1;
        }
        match context.send(self.sender.as_ref().expect("sender"), 42)? {
            SendResult::Pending(value) => {
                assert_eq!(value, 42);
                Ok(Poll::Pending)
            }
            SendResult::Closed(value) => {
                context.close_sender(self.sender.take().expect("sender"))?;
                match context.join()? {
                    Poll::Ready(()) => Ok(Poll::Ready(value)),
                    Poll::Pending => Ok(Poll::Pending),
                }
            }
            SendResult::Sent => Err("full channel accepted second message".into()),
        }
    }
}

#[test]
fn blocked_sender_wakes_closed_with_original_message() {
    let mut scheduler = Scheduler::new(RejectedBlockedSendRoot::default(), 2, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.stats().discarded_messages, 1);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct CountingChild {
    remaining: usize,
}

impl Task for CountingChild {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.remaining == 0 {
            return Ok(Poll::Ready(42));
        }
        self.remaining -= 1;
        context.yield_now()?;
        Ok(Poll::Pending)
    }
}

#[derive(Default)]
struct YieldRoot {
    child: Option<TaskId>,
}

impl Task for YieldRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.child.is_none() {
            self.child = Some(context.spawn(CountingChild { remaining: 3 }, &[])?);
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(
                context
                    .task_result(self.child.expect("child"))?
                    .expect("done"),
            )),
        }
    }
}

#[test]
fn cooperative_yields_keep_task_runnable_and_join_wakes_root() {
    let mut scheduler = Scheduler::new(YieldRoot::default(), 2, 0, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.stats().yields, 3);
    assert_eq!(scheduler.stats().parks, 1);
    scheduler.verify().unwrap();
}

struct JoiningChild {
    sibling: TaskId,
    grandchild: Option<TaskId>,
}

impl Task for JoiningChild {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.grandchild.is_none() {
            self.grandchild = Some(context.spawn(CountingChild { remaining: 0 }, &[])?);
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => {
                assert_eq!(context.task_result(self.grandchild.unwrap())?, Some(42));
                assert_eq!(context.task_result(self.sibling)?, None);
                Ok(Poll::Ready(41))
            }
        }
    }
}

#[derive(Default)]
struct NestedJoinRoot {
    child: Option<TaskId>,
    sibling: Option<TaskId>,
}

impl Task for NestedJoinRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.child.is_none() {
            let sibling = context.spawn(CountingChild { remaining: 5 }, &[])?;
            self.sibling = Some(sibling);
            self.child = Some(context.spawn(
                JoiningChild {
                    sibling,
                    grandchild: None,
                },
                &[],
            )?);
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(
                context.task_result(self.child.unwrap())?.unwrap()
                    + context.task_result(self.sibling.unwrap())?.unwrap(),
            )),
        }
    }
}

#[test]
fn child_joins_only_its_descendants_and_root_waits_for_all() {
    let mut scheduler = Scheduler::new(NestedJoinRoot::default(), 4, 0, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 83);
    assert_eq!(scheduler.stats().tasks_created, 4);
    scheduler.verify().unwrap();
}

struct FailingGrandchild;

impl Task for FailingGrandchild {
    fn poll(&mut self, _context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        Err("nested boom".into())
    }
}

#[derive(Default)]
struct JoiningFailureChild {
    spawned: bool,
}

impl Task for JoiningFailureChild {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if !self.spawned {
            context.spawn(FailingGrandchild, &[])?;
            self.spawned = true;
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Err("failed descendant should cancel before join resumes".into()),
        }
    }
}

#[derive(Default)]
struct JoiningFailureRoot {
    spawned: bool,
}

impl Task for JoiningFailureRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if !self.spawned {
            context.spawn(JoiningFailureChild::default(), &[])?;
            self.spawned = true;
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Err("failed descendant should cancel root".into()),
        }
    }
}

#[test]
fn failing_grandchild_cancels_waiting_ancestors() {
    let mut scheduler = Scheduler::new(JoiningFailureRoot::default(), 3, 0, 0).unwrap();
    assert!(
        scheduler
            .run()
            .unwrap_err()
            .contains("task 2 failed: nested boom")
    );
    scheduler.verify().unwrap();
}

struct SpawnGrandchildAndReturn;

impl Task for SpawnGrandchildAndReturn {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        context.spawn(CountingChild { remaining: 1 }, &[])?;
        Ok(Poll::Ready(1))
    }
}

#[derive(Default)]
struct LexicalScopeRoot {
    outside: Option<TaskId>,
    scope: Option<ScopeId>,
    joined_inner: bool,
}

impl Task for LexicalScopeRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.scope.is_none() {
            self.outside = Some(context.spawn(CountingChild { remaining: 5 }, &[])?);
            let scope = context.scope_enter()?;
            context.spawn(SpawnGrandchildAndReturn, &[])?;
            self.scope = Some(scope);
        }
        if !self.joined_inner {
            match context.scope_join(self.scope.unwrap())? {
                Poll::Pending => return Ok(Poll::Pending),
                Poll::Ready(()) => {
                    assert_eq!(context.task_result(self.outside.unwrap())?, None);
                    self.joined_inner = true;
                }
            }
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(
                context.task_result(self.outside.unwrap())?.unwrap(),
            )),
        }
    }
}

#[test]
fn lexical_scope_joins_transitive_children_without_waiting_for_prior_sibling() {
    let mut scheduler = Scheduler::new(LexicalScopeRoot::default(), 4, 0, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    scheduler.verify().unwrap();
}

struct ReusedScopeRoot;

impl Task for ReusedScopeRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let old = context.scope_enter()?;
        assert_eq!(context.scope_join(old)?, Poll::Ready(()));
        let current = context.scope_enter()?;
        assert_ne!(old, current);
        assert!(context.scope_join(old).is_err());
        assert_eq!(context.scope_join(current)?, Poll::Ready(()));
        Ok(Poll::Ready(42))
    }
}

#[test]
fn closed_scope_token_cannot_join_reused_slot() {
    let mut scheduler = Scheduler::new(ReusedScopeRoot, 1, 0, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    scheduler.verify().unwrap();
}

struct UnjoinedScopeRoot;

impl Task for UnjoinedScopeRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        context.scope_enter()?;
        Ok(Poll::Ready(42))
    }
}

#[test]
fn task_cannot_leave_a_nested_scope_unjoined() {
    let mut scheduler = Scheduler::new(UnjoinedScopeRoot, 1, 0, 0).unwrap();
    assert!(
        scheduler
            .run()
            .unwrap_err()
            .contains("unjoined nested scope")
    );
    scheduler.verify().unwrap();
}

#[derive(Default)]
struct NestedScopeChild {
    scope: Option<ScopeId>,
}

impl Task for NestedScopeChild {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.scope.is_none() {
            let scope = context.scope_enter()?;
            context.spawn(CountingChild { remaining: 1 }, &[])?;
            self.scope = Some(scope);
        }
        match context.scope_join(self.scope.unwrap())? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(41)),
        }
    }
}

#[derive(Default)]
struct NestedScopeRoot {
    scope: Option<ScopeId>,
}

impl Task for NestedScopeRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.scope.is_none() {
            let scope = context.scope_enter()?;
            context.spawn(NestedScopeChild::default(), &[])?;
            self.scope = Some(scope);
        }
        match context.scope_join(self.scope.unwrap())? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(42)),
        }
    }
}

#[test]
fn nested_scope_joins_wake_in_descendant_order() {
    let mut scheduler = Scheduler::new(NestedScopeRoot::default(), 3, 0, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.stats().parks, 2);
    scheduler.verify().unwrap();
}
