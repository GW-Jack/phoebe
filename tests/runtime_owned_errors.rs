use std::ffi::c_void;
use std::task::Poll;

use phoebe::runtime::{
    CompleteResult, Context, NativeCallContext, Receiver, ReplyReceiveResult, Request,
    ResourceError, Scheduler, ScopeLimits, Sender, Task, phoebe_channel_new_i64,
    phoebe_close_handle_i64, phoebe_fork_handle_i64, phoebe_receive_i64, phoebe_send_i64,
};

struct UnstartedChild {
    sender: Option<Sender>,
}

impl Task for UnstartedChild {
    fn poll(&mut self, _context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        Err("rejected child must never run".into())
    }
}

struct SpawnFailureRoot;

impl Task for SpawnFailureRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver) = context.channel(1)?;
        let token = sender.token();
        let error = context
            .spawn_owned(
                UnstartedChild {
                    sender: Some(sender),
                },
                &[token],
            )
            .expect_err("task budget rejects spawn");
        assert_eq!(error.message, "scope task limit exceeded");
        assert_eq!(error.resource_error, Some(ResourceError::TaskLimit));
        let mut child = error.value;
        context.close_sender(child.sender.take().expect("sender returned in frame"))?;
        context.close_receiver(receiver)?;
        Ok(Poll::Ready(42))
    }
}

#[test]
fn failed_spawn_returns_child_frame_and_keeps_handle_with_parent() {
    let mut scheduler = Scheduler::new(SpawnFailureRoot, 1, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.stats().tasks_created, 1);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct SendFailureRoot;

impl Task for SendFailureRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver): (Sender, Receiver) = context.channel(1)?;
        let (right, reply_receiver) = context.reply()?;
        let error = context
            .send_request_owned(
                &sender,
                Request {
                    value: 41,
                    reply: right,
                },
            )
            .expect_err("wrong channel shape rejects request");
        assert!(error.message.contains("request channel"));
        let request = error.value;
        assert_eq!(
            context.complete(request.reply, request.value + 1)?,
            CompleteResult::Delivered
        );
        let value = match context.receive_reply(&reply_receiver)? {
            ReplyReceiveResult::Value(value) => value,
            _ => return Err("recovered reply right was damaged".into()),
        };
        context.close_sender(sender)?;
        context.close_receiver(receiver)?;
        Ok(Poll::Ready(value))
    }
}

#[test]
fn failed_request_send_returns_intact_reply_right() {
    let mut scheduler = Scheduler::new(SendFailureRoot, 1, 1, 1).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct CreationFailureRoot;

impl Task for CreationFailureRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        assert_eq!(
            context.channel_typed(0).unwrap_err(),
            ResourceError::InvalidChannelCapacity
        );
        assert_eq!(
            context.channel_typed(usize::MAX).unwrap_err(),
            ResourceError::Allocation
        );
        let (sender, receiver) = context
            .channel_typed(1)
            .map_err(|error| error.to_string())?;
        assert_eq!(
            context.request_channel_typed(1).unwrap_err(),
            ResourceError::ChannelLimit
        );
        assert_eq!(
            context.reply_typed().unwrap_err(),
            ResourceError::ReplyLimit
        );
        context.close_sender(sender)?;
        context.close_receiver(receiver)?;
        Ok(Poll::Ready(42))
    }
}

#[test]
fn typed_creation_errors_preserve_resource_tables() {
    let mut scheduler = Scheduler::new(CreationFailureRoot, 1, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct NativeAbiRoot;

impl Task for NativeAbiRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let mut fault = false;
        let mut bridge = NativeCallContext {
            context: context as *mut _ as *mut c_void,
            fault: &mut fault,
            call_request: std::ptr::null_mut(),
        };
        let mut sender = 111;
        let mut receiver = 222;
        let rejected =
            unsafe { phoebe_channel_new_i64(&mut bridge, 0, &mut sender, &mut receiver) };
        assert_eq!(rejected, 1);
        assert_eq!((sender, receiver), (111, 222));
        let allocation =
            unsafe { phoebe_channel_new_i64(&mut bridge, i64::MAX, &mut sender, &mut receiver) };
        assert_eq!(allocation, 4);
        assert_eq!((sender, receiver), (111, 222));
        let accepted =
            unsafe { phoebe_channel_new_i64(&mut bridge, 1, &mut sender, &mut receiver) };
        assert_eq!(accepted, 0);
        assert_eq!(unsafe { phoebe_send_i64(&mut bridge, sender, 41) }, 0);
        let mut value = -1;
        assert_eq!(
            unsafe { phoebe_receive_i64(&mut bridge, receiver, &mut value) },
            0
        );
        assert_eq!(value, 41);
        assert_eq!(
            unsafe { phoebe_close_handle_i64(&mut bridge, sender, 0) },
            0
        );
        value = -1;
        assert_eq!(
            unsafe { phoebe_receive_i64(&mut bridge, receiver, &mut value) },
            2
        );
        assert_eq!(value, -1);
        assert_eq!(
            unsafe { phoebe_close_handle_i64(&mut bridge, receiver, 1) },
            0
        );
        assert_eq!(
            unsafe { phoebe_channel_new_i64(&mut bridge, 1, &mut sender, &mut receiver) },
            0
        );
        assert_eq!(
            unsafe { phoebe_close_handle_i64(&mut bridge, receiver, 1) },
            0
        );
        assert_eq!(unsafe { phoebe_send_i64(&mut bridge, sender, 42) }, 2);
        assert_eq!(
            unsafe { phoebe_close_handle_i64(&mut bridge, sender, 0) },
            0
        );
        assert!(!fault);
        Ok(Poll::Ready(42))
    }
}

#[test]
fn native_abi_publishes_both_handles_only_on_success() {
    let mut scheduler = Scheduler::new(NativeAbiRoot, 1, 2, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct HandleBudgetRoot;

impl Task for HandleBudgetRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver) = context.channel_typed(1).map_err(|e| e.to_string())?;
        let fork = context
            .fork_sender_typed(&sender)?
            .map_err(|e| e.to_string())?;
        assert_eq!(
            context.fork_receiver_typed(&receiver)?.unwrap_err(),
            ResourceError::HandleLimit
        );
        context.close_sender(sender)?;
        context.close_sender(fork)?;
        context.close_receiver(receiver)?;
        assert_eq!(
            context.channel_typed(1).unwrap_err(),
            ResourceError::HandleLimit
        );
        Ok(Poll::Ready(42))
    }
}

#[test]
fn handle_budget_rejects_fork_before_changing_ownership() {
    let limits = ScopeLimits {
        tasks: 1,
        channels: 2,
        handles: 3,
        replies: 0,
    };
    let mut scheduler = Scheduler::with_limits(HandleBudgetRoot, limits).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct NativeForkBudgetRoot;

impl Task for NativeForkBudgetRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let mut fault = false;
        let mut bridge = NativeCallContext {
            context: context as *mut _ as *mut c_void,
            fault: &mut fault,
            call_request: std::ptr::null_mut(),
        };
        let mut sender = -1;
        let mut receiver = -1;
        assert_eq!(
            unsafe { phoebe_channel_new_i64(&mut bridge, 1, &mut sender, &mut receiver) },
            0
        );
        let mut out = 123;
        assert_eq!(
            unsafe { phoebe_fork_handle_i64(&mut bridge, sender, 0, &mut out) },
            6
        );
        assert_eq!(out, 123);
        assert_eq!(unsafe { phoebe_send_i64(&mut bridge, sender, 42) }, 0);
        let mut value = -1;
        assert_eq!(
            unsafe { phoebe_receive_i64(&mut bridge, receiver, &mut value) },
            0
        );
        assert_eq!(value, 42);
        assert_eq!(
            unsafe { phoebe_close_handle_i64(&mut bridge, sender, 0) },
            0
        );
        assert_eq!(
            unsafe { phoebe_close_handle_i64(&mut bridge, receiver, 1) },
            0
        );
        assert!(!fault);
        Ok(Poll::Ready(value))
    }
}

#[test]
fn native_fork_budget_status_preserves_original_and_output() {
    let limits = ScopeLimits {
        tasks: 1,
        channels: 1,
        handles: 2,
        replies: 0,
    };
    let mut scheduler = Scheduler::with_limits(NativeForkBudgetRoot, limits).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}
