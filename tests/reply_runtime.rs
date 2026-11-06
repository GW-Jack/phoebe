use std::task::Poll;

use phoebe::runtime::{
    CompleteResult, Context, Receiver, ReplyReceiveResult, ReplyReceiver, Request,
    RequestReceiveResult, RequestSendResult, Scheduler, Sender, Task,
};

struct Service {
    receiver: Option<Receiver>,
}

impl Task for Service {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        match context.receive_request(self.receiver.as_ref().expect("receiver"))? {
            RequestReceiveResult::Pending => Ok(Poll::Pending),
            RequestReceiveResult::Value(request) => {
                assert_eq!(
                    context.complete(request.reply, request.value + 1)?,
                    CompleteResult::Delivered
                );
                context.close_receiver(self.receiver.take().expect("receiver"))?;
                Ok(Poll::Ready(0))
            }
            RequestReceiveResult::End => Err("service ended without request".into()),
        }
    }
}

#[derive(Default)]
struct ReplyRoot {
    receiver: Option<ReplyReceiver>,
    value: Option<i64>,
}

impl Task for ReplyRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.receiver.is_none() && self.value.is_none() {
            let (sender, inbox) = context.request_channel(1)?;
            let (reply, receiver) = context.reply()?;
            let token = inbox.token();
            context.spawn(
                Service {
                    receiver: Some(inbox),
                },
                &[token],
            )?;
            assert!(matches!(
                context.send_request(&sender, Request { value: 41, reply })?,
                RequestSendResult::Sent
            ));
            context.close_sender(sender)?;
            self.receiver = Some(receiver);
        }
        if let Some(receiver) = &self.receiver {
            match context.receive_reply(receiver)? {
                ReplyReceiveResult::Pending => return Ok(Poll::Pending),
                ReplyReceiveResult::Value(value) => {
                    self.value = Some(value);
                    self.receiver = None;
                }
                ReplyReceiveResult::End => return Err("reply was abandoned".into()),
            }
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(self.value.expect("reply received"))),
        }
    }
}

#[test]
fn request_queue_transfers_reply_right_to_service_once() {
    let mut scheduler = Scheduler::new(ReplyRoot::default(), 2, 1).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct RejectedRequestRoot;

impl Task for RejectedRequestRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver) = context.request_channel(1)?;
        let (reply, reply_receiver) = context.reply()?;
        context.close_receiver(receiver)?;
        let request = match context.send_request(&sender, Request { value: 41, reply })? {
            RequestSendResult::Closed(request) => request,
            _ => return Err("closed request channel accepted message".into()),
        };
        assert_eq!(
            context.complete(request.reply, request.value + 1)?,
            CompleteResult::Delivered
        );
        let value = match context.receive_reply(&reply_receiver)? {
            ReplyReceiveResult::Value(value) => value,
            _ => return Err("reply was not retained on rejection".into()),
        };
        context.close_sender(sender)?;
        Ok(Poll::Ready(value))
    }
}

#[test]
fn rejected_send_returns_request_with_live_reply_right() {
    let mut scheduler = Scheduler::new(RejectedRequestRoot, 1, 1).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct DiscardRequestRoot;

impl Task for DiscardRequestRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver) = context.request_channel(1)?;
        let (reply, reply_receiver) = context.reply()?;
        assert!(matches!(
            context.send_request(&sender, Request { value: 41, reply })?,
            RequestSendResult::Sent
        ));
        context.close_receiver(receiver)?;
        assert_eq!(
            context.receive_reply(&reply_receiver)?,
            ReplyReceiveResult::End
        );
        context.close_sender(sender)?;
        Ok(Poll::Ready(42))
    }
}

#[test]
fn discarding_queued_request_closes_reply_right_and_wakes_receiver() {
    let mut scheduler = Scheduler::new(DiscardRequestRoot, 1, 1).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.stats().discarded_messages, 1);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct FailingService {
    receiver: Option<Receiver>,
}

impl Task for FailingService {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        match context.receive_request(self.receiver.as_ref().expect("receiver"))? {
            RequestReceiveResult::Value(_request) => Err("service failed".into()),
            RequestReceiveResult::Pending => Ok(Poll::Pending),
            RequestReceiveResult::End => Err("request missing".into()),
        }
    }
}

#[derive(Default)]
struct FailureRoot {
    reply_receiver: Option<ReplyReceiver>,
    started: bool,
    observed_end: bool,
}

impl Task for FailureRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if !self.started {
            let (sender, receiver) = context.request_channel(1)?;
            let (reply, reply_receiver) = context.reply()?;
            let token = receiver.token();
            context.spawn(
                FailingService {
                    receiver: Some(receiver),
                },
                &[token],
            )?;
            assert!(matches!(
                context.send_request(&sender, Request { value: 41, reply })?,
                RequestSendResult::Sent
            ));
            context.close_sender(sender)?;
            self.reply_receiver = Some(reply_receiver);
            self.started = true;
        }
        if !self.observed_end {
            match context.receive_reply(self.reply_receiver.as_ref().expect("reply receiver"))? {
                ReplyReceiveResult::Pending => return Ok(Poll::Pending),
                ReplyReceiveResult::End => {
                    self.reply_receiver = None;
                    self.observed_end = true;
                }
                ReplyReceiveResult::Value(_) => return Err("failed service replied".into()),
            }
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(0)),
        }
    }
}

#[test]
fn child_failure_closes_dequeued_reply_right_and_scope_reports_failure() {
    let mut scheduler = Scheduler::new(FailureRoot::default(), 2, 1).unwrap();
    assert!(
        scheduler
            .run()
            .unwrap_err()
            .contains("task 1 failed: service failed")
    );
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct CancelRoot;

impl Task for CancelRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (reply, receiver) = context.reply()?;
        context.cancel_reply(receiver)?;
        assert_eq!(context.complete(reply, 42)?, CompleteResult::Closed(42));
        Ok(Poll::Ready(42))
    }
}

#[test]
fn canceled_reply_receiver_rejects_late_completion() {
    let mut scheduler = Scheduler::new(CancelRoot, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct ClosingService {
    receiver: Option<Receiver>,
}

impl Task for ClosingService {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        context.close_receiver(self.receiver.take().expect("receiver"))?;
        Ok(Poll::Ready(0))
    }
}

#[derive(Default)]
struct BackpressureRoot {
    sender: Option<Sender>,
    pending: Option<Request>,
    first_reply: Option<ReplyReceiver>,
    second_reply: Option<ReplyReceiver>,
}

impl Task for BackpressureRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.sender.is_none() {
            let (sender, receiver) = context.request_channel(1)?;
            let token = receiver.token();
            context.spawn(
                ClosingService {
                    receiver: Some(receiver),
                },
                &[token],
            )?;
            let (first, first_reply) = context.reply()?;
            let (second, second_reply) = context.reply()?;
            assert!(matches!(
                context.send_request(
                    &sender,
                    Request {
                        value: 1,
                        reply: first
                    }
                )?,
                RequestSendResult::Sent
            ));
            self.pending = Some(
                match context.send_request(
                    &sender,
                    Request {
                        value: 41,
                        reply: second,
                    },
                )? {
                    RequestSendResult::Pending(request) => request,
                    _ => return Err("full request queue did not park sender".into()),
                },
            );
            self.sender = Some(sender);
            self.first_reply = Some(first_reply);
            self.second_reply = Some(second_reply);
            return Ok(Poll::Pending);
        }
        let request = self.pending.take().expect("retained request");
        let sender = self.sender.as_ref().expect("sender");
        let request = match context.send_request(sender, request)? {
            RequestSendResult::Closed(request) => request,
            _ => return Err("closed request channel did not return pending request".into()),
        };
        assert_eq!(
            context.receive_reply(self.first_reply.as_ref().expect("first reply"))?,
            ReplyReceiveResult::End
        );
        self.first_reply = None;
        assert_eq!(
            context.complete(request.reply, request.value + 1)?,
            CompleteResult::Delivered
        );
        let value =
            match context.receive_reply(self.second_reply.as_ref().expect("second reply"))? {
                ReplyReceiveResult::Value(value) => value,
                _ => return Err("pending reply right was lost".into()),
            };
        self.second_reply = None;
        context.close_sender(self.sender.take().expect("sender"))?;
        match context.join()? {
            Poll::Ready(()) => Ok(Poll::Ready(value)),
            Poll::Pending => Err("closed child did not finish".into()),
        }
    }
}

#[test]
fn full_queue_keeps_unsent_reply_right_with_sender_until_retry() {
    let mut scheduler = Scheduler::new(BackpressureRoot::default(), 2, 1).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert_eq!(scheduler.stats().parks, 1);
    assert_eq!(scheduler.stats().discarded_messages, 1);
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();
}

struct WrongShapeRoot;

impl Task for WrongShapeRoot {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver) = context.request_channel(1)?;
        assert!(
            context
                .send(&sender, 1)
                .unwrap_err()
                .contains("I64 channel")
        );
        context.close_sender(sender)?;
        context.close_receiver(receiver)?;
        Ok(Poll::Ready(0))
    }
}

#[test]
fn scalar_send_cannot_corrupt_request_queue() {
    let mut scheduler = Scheduler::new(WrongShapeRoot, 1, 1).unwrap();
    assert_eq!(scheduler.run().unwrap(), 0);
    scheduler.verify().unwrap();
}
