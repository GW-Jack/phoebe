//! Preallocated fixed-width FIFO storage for concrete channel messages.

use super::ResourceError;

pub(super) struct TypedQueue {
    type_id: u64,
    width: usize,
    capacity: usize,
    words: Vec<u64>,
    head: usize,
    len: usize,
}

impl TypedQueue {
    pub(super) fn new(type_id: u64, width: usize, capacity: usize) -> Result<Self, ResourceError> {
        if capacity == 0 {
            return Err(ResourceError::InvalidChannelCapacity);
        }
        let count = width
            .checked_mul(capacity)
            .ok_or(ResourceError::InvalidChannelCapacity)?;
        let mut words = Vec::new();
        words
            .try_reserve_exact(count)
            .map_err(|_| ResourceError::Allocation)?;
        words.resize(count, 0);
        Ok(Self {
            type_id,
            width,
            capacity,
            words,
            head: 0,
            len: 0,
        })
    }

    pub(super) fn matches(&self, type_id: u64, width: usize) -> bool {
        self.type_id == type_id && self.width == width
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn capacity(&self) -> usize {
        self.capacity
    }

    pub(super) fn is_full(&self) -> bool {
        self.len == self.capacity
    }

    pub(super) fn push(&mut self, value: &[u64]) {
        debug_assert_eq!(value.len(), self.width);
        debug_assert!(!self.is_full());
        let tail = if self.head >= self.capacity - self.len {
            self.head - (self.capacity - self.len)
        } else {
            self.head + self.len
        };
        let start = tail * self.width;
        self.words[start..start + self.width].copy_from_slice(value);
        self.len += 1;
    }

    pub(super) fn pop(&mut self, output: &mut [u64]) -> bool {
        debug_assert_eq!(output.len(), self.width);
        if self.len == 0 {
            return false;
        }
        let start = self.head * self.width;
        output.copy_from_slice(&self.words[start..start + self.width]);
        self.head = (self.head + 1) % self.capacity;
        self.len -= 1;
        true
    }

    pub(super) fn discard(&mut self) -> usize {
        let count = self.len;
        self.len = 0;
        self.head = 0;
        count
    }

    pub(super) fn verify(&self) -> bool {
        self.capacity > 0
            && self.head < self.capacity
            && self.len <= self.capacity
            && self
                .width
                .checked_mul(self.capacity)
                .is_some_and(|count| count == self.words.len())
    }
}

#[cfg(test)]
mod tests {
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
    }

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
}
