use super::*;

#[test]
fn fifo_wraps_and_zero_width_messages_still_use_capacity() {
    let mut queue = TypedQueue::new(7, 2, 2).unwrap();
    let mut output = [0; 2];
    queue.push(&[1, 2]);
    queue.push(&[3, 4]);
    assert!(queue.is_full());
    assert!(queue.pop(&mut output));
    assert_eq!(output, [1, 2]);
    queue.push(&[5, 6]);
    assert!(queue.pop(&mut output));
    assert_eq!(output, [3, 4]);
    assert!(queue.pop(&mut output));
    assert_eq!(output, [5, 6]);
    assert!(!queue.pop(&mut output));
    assert!(queue.verify());

    let mut units = TypedQueue::new(8, 0, 1).unwrap();
    units.push(&[]);
    assert!(units.is_full());
    assert!(units.pop(&mut []));
    assert_eq!(units.len(), 0);
    assert!(units.verify());
}

#[test]
fn invalid_extent_is_rejected_before_storage_allocation() {
    assert!(matches!(
        TypedQueue::new(1, 0, 0),
        Err(ResourceError::InvalidChannelCapacity)
    ));
    assert!(matches!(
        TypedQueue::new(1, usize::MAX, 2),
        Err(ResourceError::InvalidChannelCapacity)
    ));
    let mut deep = TypedRightPlan::ReplyTo { part: 0 };
    for _ in 0..64 {
        deep = TypedRightPlan::Sequence(vec![deep]);
    }
    assert!(matches!(
        TypedQueue::with_plan(1, 1, 1, deep),
        Err(ResourceError::InvalidChannelCapacity)
    ));
}

#[derive(Debug)]
struct Idle;

impl super::super::Task for Idle {
    fn poll(
        &mut self,
        _context: &mut super::super::Context<'_>,
    ) -> Result<std::task::Poll<i64>, String> {
        Ok(std::task::Poll::Ready(0))
    }
}

#[test]
fn scheduler_enforces_type_identity_and_discards_closed_typed_queue() {
    use super::super::{Context, Scheduler, TaskId, TypedReceiveResult, TypedSendResult};

    let mut scheduler = Scheduler::new(Idle, 1, 1, 0).unwrap();
    {
        let mut context = Context {
            scheduler: &mut scheduler,
            task: TaskId(0),
            wait: None,
        };
        let (sender, receiver) = context.typed_words_channel(2, 41, 2).unwrap();
        assert!(context.send_typed_words(&sender, 42, &[1, 2]).is_err());
        assert!(context.send_typed_words(&sender, 41, &[1]).is_err());
        assert_eq!(
            context.send_typed_words(&sender, 41, &[1, 2]).unwrap(),
            TypedSendResult::Sent
        );
        assert_eq!(
            context.send_typed_words(&sender, 41, &[3, 4]).unwrap(),
            TypedSendResult::Sent
        );
        let mut output = [0; 2];
        assert!(
            context
                .receive_typed_words(&receiver, 42, &mut output)
                .is_err()
        );
        assert_eq!(output, [0, 0]);
        assert_eq!(
            context
                .receive_typed_words(&receiver, 41, &mut output)
                .unwrap(),
            TypedReceiveResult::Value
        );
        assert_eq!(output, [1, 2]);
        context.close_receiver(receiver).unwrap();
        assert_eq!(
            context.send_typed_words(&sender, 41, &[5, 6]).unwrap(),
            TypedSendResult::Closed
        );
        context.close_sender(sender).unwrap();
    }
    assert_eq!(scheduler.stats().discarded_messages, 1);
    scheduler.verify().unwrap();
}

#[test]
fn full_typed_send_leaves_callers_words_unchanged() {
    use super::super::{Context, Scheduler, TaskId, TypedSendResult};

    let mut scheduler = Scheduler::new(Idle, 1, 1, 0).unwrap();
    let mut context = Context {
        scheduler: &mut scheduler,
        task: TaskId(0),
        wait: None,
    };
    let (sender, _receiver) = context.typed_words_channel(1, 9, 0).unwrap();
    assert_eq!(
        context.send_typed_words(&sender, 9, &[]).unwrap(),
        TypedSendResult::Sent
    );
    let retained = [];
    assert_eq!(
        context.send_typed_words(&sender, 9, &retained).unwrap(),
        TypedSendResult::Pending
    );
    assert!(retained.is_empty());
}

struct TypedReader {
    receiver: super::super::Receiver,
    seen: usize,
}

impl super::super::Task for TypedReader {
    fn poll(
        &mut self,
        context: &mut super::super::Context<'_>,
    ) -> Result<std::task::Poll<i64>, String> {
        use super::super::TypedReceiveResult;
        loop {
            let mut words = [0; 2];
            match context.receive_typed_words(&self.receiver, 77, &mut words)? {
                TypedReceiveResult::Value => {
                    self.seen += 1;
                    assert_eq!(words, [self.seen as u64, 10 + self.seen as u64]);
                }
                TypedReceiveResult::Pending => return Ok(std::task::Poll::Pending),
                TypedReceiveResult::End => {
                    assert_eq!(self.seen, 2);
                    context.close_receiver(super::super::Receiver::from_token(
                        self.receiver.token(),
                    ))?;
                    return Ok(std::task::Poll::Ready(0));
                }
            }
        }
    }
}

struct TypedWriter {
    sender: Option<super::super::Sender>,
    stage: u8,
}

impl super::super::Task for TypedWriter {
    fn poll(
        &mut self,
        context: &mut super::super::Context<'_>,
    ) -> Result<std::task::Poll<i64>, String> {
        use super::super::TypedSendResult;
        if self.stage == 0 {
            let (sender, receiver) = context
                .typed_words_channel(1, 77, 2)
                .map_err(|error| error.to_string())?;
            let token = receiver.token();
            context
                .spawn_owned(TypedReader { receiver, seen: 0 }, &[token])
                .map_err(|error| error.message)?;
            assert_eq!(
                context.send_typed_words(&sender, 77, &[1, 11])?,
                TypedSendResult::Sent
            );
            self.sender = Some(sender);
            self.stage = 1;
        }
        if self.stage == 1 {
            let sender = self.sender.as_ref().unwrap();
            match context.send_typed_words(sender, 77, &[2, 12])? {
                TypedSendResult::Sent => {
                    context.close_sender(self.sender.take().unwrap())?;
                    self.stage = 2;
                }
                TypedSendResult::Pending => return Ok(std::task::Poll::Pending),
                TypedSendResult::Closed => panic!("reader still owns receiver"),
            }
        }
        match context.join()? {
            std::task::Poll::Ready(()) => Ok(std::task::Poll::Ready(42)),
            std::task::Poll::Pending => Ok(std::task::Poll::Pending),
        }
    }
}

#[test]
fn full_typed_channel_parks_then_wakes_both_tasks() {
    use super::super::Scheduler;
    let mut scheduler = Scheduler::new(
        TypedWriter {
            sender: None,
            stage: 0,
        },
        2,
        1,
        0,
    )
    .unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert!(scheduler.stats().parks >= 2);
    scheduler.verify().unwrap();
}

#[test]
fn multi_right_send_validates_all_before_transferring_any() {
    use super::super::{
        Context, ReplyOwner, ReplyTo, Scheduler, TaskId, TypedReceiveResult, TypedSendResult,
    };

    let mut scheduler = Scheduler::new(Idle, 1, 1, 2).unwrap();
    let (sender, receiver, first, second, first_receiver, second_receiver) = {
        let mut context = Context {
            scheduler: &mut scheduler,
            task: TaskId(0),
            wait: None,
        };
        let plan = TypedRightPlan::Sequence(vec![
            TypedRightPlan::ReplyTo { part: 0 },
            TypedRightPlan::ReplyTo { part: 1 },
        ]);
        let (sender, receiver) = context.typed_right_channel(1, 91, 2, plan).unwrap();
        let (first, first_receiver) = context.reply().unwrap();
        let (second, second_receiver) = context.reply().unwrap();
        let first_word = first.token().0 as u64;
        let second_word = second.token().0 as u64;
        assert!(
            context
                .send_typed_words(&sender, 91, &[first_word, 999])
                .is_err()
        );
        assert!(
            context
                .send_typed_words(&sender, 91, &[first_word, first_word])
                .is_err()
        );
        assert_eq!(
            context.send_typed_words(&sender, 91, &[first_word, second_word]),
            Ok(TypedSendResult::Sent)
        );
        (
            sender,
            receiver,
            first,
            second,
            first_receiver,
            second_receiver,
        )
    };
    assert_eq!(
        scheduler.reply_rights[first.token().0].owner,
        ReplyOwner::Queue(0)
    );
    assert_eq!(
        scheduler.reply_rights[second.token().0].owner,
        ReplyOwner::Queue(0)
    );
    scheduler.verify().unwrap();
    {
        let mut context = Context {
            scheduler: &mut scheduler,
            task: TaskId(0),
            wait: None,
        };
        let mut words = [0; 2];
        assert_eq!(
            context.receive_typed_words(&receiver, 91, &mut words),
            Ok(TypedReceiveResult::Value)
        );
        assert_eq!(words, [first.token().0 as u64, second.token().0 as u64]);
        context
            .close_reply_to(ReplyTo::from_token(first.token()))
            .unwrap();
        context
            .close_reply_to(ReplyTo::from_token(second.token()))
            .unwrap();
        context.cancel_reply(first_receiver).unwrap();
        context.cancel_reply(second_receiver).unwrap();
        context.close_sender(sender).unwrap();
        context.close_receiver(receiver).unwrap();
    }
    scheduler.verify().unwrap();
    assert_eq!(scheduler.open_handles(), 0);
}

#[test]
fn inactive_tag_ignores_payload_and_discard_closes_active_rights() {
    use super::super::{
        Context, ReplyOwner, ReplyReceiveResult, Scheduler, TaskId, TypedReceiveResult,
        TypedSendResult,
    };

    let mut scheduler = Scheduler::new(Idle, 1, 1, 2).unwrap();
    let (sender, receiver, first_receiver, second_receiver, first_token, second_token) = {
        let mut context = Context {
            scheduler: &mut scheduler,
            task: TaskId(0),
            wait: None,
        };
        let plan = TypedRightPlan::Tagged {
            tag_part: 0,
            arms: vec![
                TypedRightPlan::Empty,
                TypedRightPlan::Sequence(vec![
                    TypedRightPlan::ReplyTo { part: 1 },
                    TypedRightPlan::ReplyTo { part: 2 },
                ]),
            ],
        };
        let (sender, receiver) = context.typed_right_channel(2, 92, 3, plan).unwrap();
        let (first, first_receiver) = context.reply().unwrap();
        let (second, second_receiver) = context.reply().unwrap();
        let first_token = first.token();
        let second_token = second.token();
        assert_eq!(
            context.send_typed_words(&sender, 92, &[0, 999, 999]),
            Ok(TypedSendResult::Sent)
        );
        assert!(
            context
                .send_typed_words(
                    &sender,
                    92,
                    &[2, first_token.0 as u64, second_token.0 as u64]
                )
                .is_err()
        );
        assert_eq!(
            context.send_typed_words(
                &sender,
                92,
                &[1, first_token.0 as u64, second_token.0 as u64]
            ),
            Ok(TypedSendResult::Sent)
        );
        (
            sender,
            receiver,
            first_receiver,
            second_receiver,
            first_token,
            second_token,
        )
    };
    scheduler.verify().unwrap();
    {
        let mut context = Context {
            scheduler: &mut scheduler,
            task: TaskId(0),
            wait: None,
        };
        let mut words = [0; 3];
        assert_eq!(
            context.receive_typed_words(&receiver, 92, &mut words),
            Ok(TypedReceiveResult::Value)
        );
        assert_eq!(words, [0, 999, 999]);
        context.close_receiver(receiver).unwrap();
        assert_eq!(
            context.receive_reply(&first_receiver),
            Ok(ReplyReceiveResult::End)
        );
        assert_eq!(
            context.receive_reply(&second_receiver),
            Ok(ReplyReceiveResult::End)
        );
        context.close_sender(sender).unwrap();
    }
    assert_eq!(
        scheduler.reply_rights[first_token.0].owner,
        ReplyOwner::Closed
    );
    assert_eq!(
        scheduler.reply_rights[second_token.0].owner,
        ReplyOwner::Closed
    );
    assert_eq!(scheduler.stats().discarded_messages, 1);
    scheduler.verify().unwrap();
}

#[test]
fn pending_and_closed_typed_sends_retain_unsent_reply_right() {
    use super::super::{Context, ReplyOwner, Scheduler, TaskId, TypedSendResult};

    let mut scheduler = Scheduler::new(Idle, 1, 1, 2).unwrap();
    let (queued, unsent) = {
        let mut context = Context {
            scheduler: &mut scheduler,
            task: TaskId(0),
            wait: None,
        };
        let (sender, receiver) = context
            .typed_right_channel(1, 93, 1, TypedRightPlan::ReplyTo { part: 0 })
            .unwrap();
        let (queued, queued_receiver) = context.reply().unwrap();
        let (unsent, unsent_receiver) = context.reply().unwrap();
        assert_eq!(
            context.send_typed_words(&sender, 93, &[queued.token().0 as u64]),
            Ok(TypedSendResult::Sent)
        );
        assert_eq!(
            context.send_typed_words(&sender, 93, &[unsent.token().0 as u64]),
            Ok(TypedSendResult::Pending)
        );
        assert_eq!(
            context.scheduler.reply_rights[unsent.token().0].owner,
            ReplyOwner::Task(TaskId(0))
        );
        context.close_receiver(receiver).unwrap();
        assert_eq!(
            context.send_typed_words(&sender, 93, &[unsent.token().0 as u64]),
            Ok(TypedSendResult::Closed)
        );
        assert_eq!(
            context.scheduler.reply_rights[unsent.token().0].owner,
            ReplyOwner::Task(TaskId(0))
        );
        context.close_reply_to(unsent).unwrap();
        context.cancel_reply(queued_receiver).unwrap();
        context.cancel_reply(unsent_receiver).unwrap();
        context.close_sender(sender).unwrap();
        (queued.token(), context.scheduler.stats().discarded_messages)
    };
    assert_eq!(unsent, 1);
    assert_eq!(scheduler.reply_rights[queued.0].owner, ReplyOwner::Closed);
    scheduler.verify().unwrap();
}

#[test]
fn typed_reply_right_moves_from_parent_queue_to_child() {
    use super::super::{
        Context, Receiver, ReplyOwner, ReplyReceiveResult, ReplyTo, Scheduler, TaskId,
        TypedReceiveResult, TypedSendResult,
    };

    let mut scheduler = Scheduler::new(Idle, 2, 1, 1).unwrap();
    let (sender, receiver_token, reply_receiver, reply_token, child) = {
        let mut parent = Context {
            scheduler: &mut scheduler,
            task: TaskId(0),
            wait: None,
        };
        let (sender, receiver) = parent
            .typed_right_channel(1, 94, 1, TypedRightPlan::ReplyTo { part: 0 })
            .unwrap();
        let receiver_token = receiver.token();
        let child = parent.spawn_owned(Idle, &[receiver_token]).unwrap();
        let (right, reply_receiver) = parent.reply().unwrap();
        let reply_token = right.token();
        assert_eq!(
            parent.send_typed_words(&sender, 94, &[reply_token.0 as u64]),
            Ok(TypedSendResult::Sent)
        );
        (sender, receiver_token, reply_receiver, reply_token, child)
    };
    assert_eq!(
        scheduler.reply_rights[reply_token.0].owner,
        ReplyOwner::Queue(0)
    );
    {
        let mut child_context = Context {
            scheduler: &mut scheduler,
            task: child,
            wait: None,
        };
        let receiver = Receiver::from_token(receiver_token);
        let mut word = [0];
        assert_eq!(
            child_context.receive_typed_words(&receiver, 94, &mut word),
            Ok(TypedReceiveResult::Value)
        );
        assert_eq!(word, [reply_token.0 as u64]);
        assert_eq!(
            child_context.scheduler.reply_rights[reply_token.0].owner,
            ReplyOwner::Task(child)
        );
        child_context
            .close_reply_to(ReplyTo::from_token(reply_token))
            .unwrap();
        child_context.close_receiver(receiver).unwrap();
    }
    {
        let mut parent = Context {
            scheduler: &mut scheduler,
            task: TaskId(0),
            wait: None,
        };
        assert_eq!(
            parent.receive_reply(&reply_receiver),
            Ok(ReplyReceiveResult::End)
        );
        parent.close_sender(sender).unwrap();
    }
    scheduler.verify().unwrap();
}
