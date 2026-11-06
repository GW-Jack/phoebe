use std::task::Poll;

use phoebe::runtime::{
    Context, ReceiveResult, Receiver, Scheduler, SendResult, Sender, Task, TaskId,
};

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
    let mut scheduler = Scheduler::new(ReceiveRoot::default(), 2, 1).unwrap();
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
    let mut scheduler = Scheduler::new(BackpressureRoot::default(), 2, 1).unwrap();
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
    let mut scheduler = Scheduler::new(ClosedRoot, 1, 1).unwrap();
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
fn child_failure_closes_owned_sender_and_reports_at_scope_join() {
    let mut scheduler = Scheduler::new(FailureRoot::default(), 2, 1).unwrap();
    assert!(scheduler.run().unwrap_err().contains("task 1 failed: boom"));
    assert!(scheduler.stats().wakes >= 1);
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
    let mut scheduler = Scheduler::new(DeadlockRoot::default(), 1, 1).unwrap();
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
    let mut scheduler = Scheduler::new(RejectedBlockedSendRoot::default(), 2, 1).unwrap();
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
    let mut scheduler = Scheduler::new(YieldRoot::default(), 2, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.stats().yields, 3);
    assert_eq!(scheduler.stats().parks, 1);
    scheduler.verify().unwrap();
}
