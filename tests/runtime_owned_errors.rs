use std::task::Poll;

use phoebe::runtime::{
    CompleteResult, Context, Receiver, ReplyReceiveResult, Request, ResourceError, Scheduler,
    Sender, Task,
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
