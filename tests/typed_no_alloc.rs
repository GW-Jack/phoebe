//! A one-test binary so the allocation counter cannot observe other tests.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;

use phoebe::runtime::{
    Context, ReplyTo, Scheduler, Task, TypedReceiveResult, TypedRightPlan, TypedSendResult,
};

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct CountedSystem;

#[global_allocator]
static ALLOCATOR: CountedSystem = CountedSystem;

unsafe impl GlobalAlloc for CountedSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

struct Root;

impl Task for Root {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver) = context
            .typed_right_channel(1, 700, 2, TypedRightPlan::ReplyTo { part: 1 })
            .map_err(|error| error.to_string())?;
        let (right, answer) = context.reply()?;
        let token = right.token();
        let message = [42, token.native_word()? as u64];
        let mut received = [0; 2];

        ALLOCATIONS.store(0, Ordering::Relaxed);
        COUNTING.store(true, Ordering::Relaxed);
        let sent = context.send_typed_words(&sender, 700, &message);
        let send_allocations = ALLOCATIONS.load(Ordering::Relaxed);
        COUNTING.store(false, Ordering::Relaxed);
        assert_eq!(sent?, TypedSendResult::Sent);
        assert_eq!(send_allocations, 0, "successful typed send allocated");

        ALLOCATIONS.store(0, Ordering::Relaxed);
        COUNTING.store(true, Ordering::Relaxed);
        let outcome = context.receive_typed_words(&receiver, 700, &mut received);
        let receive_allocations = ALLOCATIONS.load(Ordering::Relaxed);
        COUNTING.store(false, Ordering::Relaxed);
        assert_eq!(outcome?, TypedReceiveResult::Value);
        assert_eq!(receive_allocations, 0, "successful typed receive allocated");

        assert_eq!(received, message);
        context.close_reply_to(ReplyTo::from_token(token))?;
        context.cancel_reply(answer)?;
        context.close_sender(sender)?;
        context.close_receiver(receiver)?;
        Ok(Poll::Ready(received[0] as i64))
    }
}

#[test]
fn successful_typed_message_transfer_does_not_allocate() {
    let mut scheduler = Scheduler::new(Root, 1, 1, 1).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    scheduler.verify().unwrap();
}
