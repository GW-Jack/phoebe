use super::*;
use phoebe_runtime::{NativeFrameInit, NativePollTask, ReceiveResult, Receiver, Sender};
use std::ffi::c_void;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

const PRODUCER_SOURCE: &str = r#"
fn producer(sender: Sender<I64>) -> I64 {
  val first = sender.send(20);
  val second = when (first) {
    Send<I64>.Sent -> { sender.send(22) },
    Send<I64>.Closed(unsent) -> { Send<I64>.Closed(unsent) }
  };
  sender.close();
  when (second) {
    Send<I64>.Sent -> { 0 },
    Send<I64>.Closed(unsent) -> { unsent }
  }
}
"#;

const SOURCE: &str = r#"
fn wait(receiver: Receiver<I64>, seed: I64) -> I64 {
  val marker = seed + 1;
  val item = receiver.receive();
  receiver.close();
  when (item) {
    Receive<I64>.Value(value) -> { marker + value },
    Receive<I64>.End -> { marker }
  }
}
fn main() -> I64 = 0
"#;

struct Capture {
    task: NativePollTask,
    answer: Arc<AtomicI64>,
}

impl Task for Capture {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let result = self.task.poll(context)?;
        if let Poll::Ready(value) = result {
            self.answer.store(value, Ordering::SeqCst);
        }
        Ok(result)
    }
}

struct Host {
    entry: runtime::NativePollEntry,
    frame_words: usize,
    receiver_word: usize,
    seed_word: usize,
    answer: Arc<AtomicI64>,
    sender: Option<Sender>,
    phase: u8,
    close_without_value: bool,
}

impl Task for Host {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.phase == 0 {
            let (sender, receiver) = context.channel(1)?;
            let token = receiver.token();
            let mut task = unsafe { NativePollTask::new(self.entry, self.frame_words) }
                .map_err(|error| error.to_string())?;
            task.write_token(self.receiver_word, token)?;
            task.frame_mut()[self.seed_word] = 7;
            context
                .spawn_owned(
                    Capture {
                        task,
                        answer: self.answer.clone(),
                    },
                    &[token],
                )
                .map_err(|error| error.message)?;
            self.sender = Some(sender);
            self.phase = 1;
            context.yield_now()?;
            return Ok(Poll::Pending);
        }
        if self.phase == 1 {
            if !self.close_without_value {
                context.send(self.sender.as_ref().expect("sender"), 42)?;
            }
            context.close_sender(self.sender.take().expect("sender"))?;
            self.phase = 2;
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(self.answer.load(Ordering::SeqCst))),
        }
    }
}

#[test]
fn generated_receive_leaf_parks_resumes_and_handles_end() {
    let program = core::lower(&phoebe_syntax::syntax::parse(SOURCE).unwrap()).unwrap();
    let executable = compile_poll_leaf_jit(&program, "wait").unwrap();
    let layout = executable.layout();
    assert_eq!(layout.entry_params.len(), 2);
    assert_eq!(layout.values.len(), 2);
    assert!(executable.clif().contains("call"));
    for (close_without_value, expected) in [(false, 50), (true, 8)] {
        let answer = Arc::new(AtomicI64::new(-1));
        let host = Host {
            entry: unsafe { executable.entry() },
            frame_words: layout.total_words,
            receiver_word: layout.entry_params[0].words.start,
            seed_word: layout.entry_params[1].words.start,
            answer,
            sender: None,
            phase: 0,
            close_without_value,
        };
        let mut scheduler = Scheduler::new(host, 2, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), expected);
        assert!(scheduler.stats().parks >= 1);
    }
}

#[test]
fn generated_call_resumes_parent_after_receiving_child() {
    let source = SOURCE.replace(
        "fn main() -> I64 = 0",
        r#"
fn relay(receiver: Receiver<I64>, seed: I64) -> I64 {
  val before = seed + 2;
  val answer = wait(receiver, seed);
  answer + before
}
fn main() -> I64 = 0
"#,
    );
    let program = core::lower(&phoebe_syntax::syntax::parse(&source).unwrap()).unwrap();
    let executable = compile_poll_jit(&program, "relay").unwrap();
    let layout = executable.layout();
    assert_eq!(layout.resumes.len(), 1);
    assert!(layout.resumes[0].instruction.is_some());
    assert_eq!(layout.call_result.as_ref().map(|slot| slot.len()), Some(1));
    for (close_without_value, expected) in [(false, 59), (true, 17)] {
        let answer = Arc::new(AtomicI64::new(-1));
        let host = Host {
            entry: unsafe { executable.entry() },
            frame_words: layout.total_words,
            receiver_word: layout.entry_params[0].words.start,
            seed_word: layout.entry_params[1].words.start,
            answer,
            sender: None,
            phase: 0,
            close_without_value,
        };
        let mut scheduler = Scheduler::new(host, 2, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), expected);
        assert!(scheduler.stats().parks >= 1);
        scheduler.verify().unwrap();
    }
}

#[test]
fn generated_call_returns_flattened_result_after_park() {
    let source = r#"
fn read(receiver: Receiver<I64>) -> Result<I64, Bool> {
  val item = receiver.receive();
  receiver.close();
  when (item) {
    Receive<I64>.Value(value) -> { Result<I64, Bool>.Ok(value) },
    Receive<I64>.End -> { Result<I64, Bool>.Err(false) }
  }
}
fn relay(receiver: Receiver<I64>, seed: I64) -> I64 {
  val result = read(receiver);
  when (result) {
    Result<I64, Bool>.Ok(value) -> { value + seed },
    Result<I64, Bool>.Err(flag) -> { seed }
  }
}
"#;
    let program = core::lower(&phoebe_syntax::syntax::parse(source).unwrap()).unwrap();
    let executable = compile_poll_jit(&program, "relay").unwrap();
    let layout = executable.layout();
    assert_eq!(layout.call_result.as_ref().map(|slot| slot.len()), Some(2));
    for (close_without_value, expected) in [(false, 49), (true, 7)] {
        let host = Host {
            entry: unsafe { executable.entry() },
            frame_words: layout.total_words,
            receiver_word: layout.entry_params[0].words.start,
            seed_word: layout.entry_params[1].words.start,
            answer: Arc::new(AtomicI64::new(-1)),
            sender: None,
            phase: 0,
            close_without_value,
        };
        let mut scheduler = Scheduler::new(host, 2, 1, 0).unwrap();
        assert_eq!(scheduler.run().unwrap(), expected);
        scheduler.verify().unwrap();
    }
}

struct ProducerHost {
    entry: runtime::NativePollEntry,
    frame_words: usize,
    sender_word: usize,
    receiver: Option<Receiver>,
    received: Vec<i64>,
    phase: u8,
}

impl Task for ProducerHost {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if self.phase == 0 {
            let (sender, receiver) = context.channel(1)?;
            let token = sender.token();
            let mut fault = false;
            let mut bridge = NativeCallContext {
                context: context as *mut _ as *mut c_void,
                fault: &mut fault,
                call_request: std::ptr::null_mut(),
            };
            let token_word = token.native_word()?;
            let initial = [NativeFrameInit {
                word: self.sender_word as i64,
                value: token_word,
            }];
            let transfers = [token_word];
            let code = unsafe {
                runtime::phoebe_spawn_poll(
                    &mut bridge,
                    self.entry,
                    self.frame_words as i64,
                    initial.as_ptr(),
                    initial.len() as i64,
                    transfers.as_ptr(),
                    transfers.len() as i64,
                )
            };
            if fault || code != 0 {
                return Err(format!("native spawn failed: {code}"));
            }
            self.receiver = Some(receiver);
            self.phase = 1;
        }
        if self.phase == 1 {
            loop {
                match context.receive(self.receiver.as_ref().expect("receiver"))? {
                    ReceiveResult::Value(value) => self.received.push(value),
                    ReceiveResult::Pending => return Ok(Poll::Pending),
                    ReceiveResult::End => {
                        context.close_receiver(self.receiver.take().expect("receiver"))?;
                        self.phase = 2;
                        break;
                    }
                }
            }
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => {
                assert_eq!(self.received, [20, 22]);
                Ok(Poll::Ready(self.received.iter().sum()))
            }
        }
    }
}

#[test]
fn generated_two_send_leaf_retries_after_one_slot_backpressure() {
    let program = core::lower(&phoebe_syntax::syntax::parse(PRODUCER_SOURCE).unwrap()).unwrap();
    let executable = compile_poll_leaf_jit(&program, "producer").unwrap();
    assert_eq!(executable.layout().resumes.len(), 2);
    let host = ProducerHost {
        entry: unsafe { executable.entry() },
        frame_words: executable.layout().total_words,
        sender_word: executable.layout().entry_params[0].words.start,
        receiver: None,
        received: Vec::new(),
        phase: 0,
    };
    let mut scheduler = Scheduler::new(host, 2, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 42);
    assert!(scheduler.stats().parks >= 2);
    assert!(scheduler.stats().wakes >= 2);
    scheduler.verify().unwrap();
}

struct FailedSpawnHost {
    entry: runtime::NativePollEntry,
    frame_words: usize,
    sender_word: usize,
}

impl Task for FailedSpawnHost {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        let (sender, receiver) = context.channel(1)?;
        let token_word = sender.token().native_word()?;
        let initial = [NativeFrameInit {
            word: self.sender_word as i64,
            value: token_word,
        }];
        let transfers = [token_word];
        let mut fault = false;
        let mut bridge = NativeCallContext {
            context: context as *mut _ as *mut c_void,
            fault: &mut fault,
            call_request: std::ptr::null_mut(),
        };
        let code = unsafe {
            runtime::phoebe_spawn_poll(
                &mut bridge,
                self.entry,
                self.frame_words as i64,
                initial.as_ptr(),
                initial.len() as i64,
                transfers.as_ptr(),
                transfers.len() as i64,
            )
        };
        assert_eq!(code, 5);
        assert!(!fault);
        context.close_sender(sender)?;
        context.close_receiver(receiver)?;
        Ok(Poll::Ready(0))
    }
}

#[test]
fn native_spawn_limit_keeps_parent_rights() {
    let program = core::lower(&phoebe_syntax::syntax::parse(PRODUCER_SOURCE).unwrap()).unwrap();
    let executable = compile_poll_leaf_jit(&program, "producer").unwrap();
    let host = FailedSpawnHost {
        entry: unsafe { executable.entry() },
        frame_words: executable.layout().total_words,
        sender_word: executable.layout().entry_params[0].words.start,
    };
    let mut scheduler = Scheduler::new(host, 1, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 0);
    assert_eq!(scheduler.stats().tasks_created, 1);
    scheduler.verify().unwrap();
}

struct ClosedSendHost {
    entry: runtime::NativePollEntry,
    frame_words: usize,
    sender_word: usize,
    child_return: Arc<AtomicI64>,
    started: bool,
}

impl Task for ClosedSendHost {
    fn poll(&mut self, context: &mut Context<'_>) -> Result<Poll<i64>, String> {
        if !self.started {
            let (sender, receiver) = context.channel(1)?;
            let token = sender.token();
            let mut child = unsafe { NativePollTask::new(self.entry, self.frame_words) }
                .map_err(|error| error.to_string())?;
            child.write_token(self.sender_word, token)?;
            context
                .spawn_owned(
                    Capture {
                        task: child,
                        answer: self.child_return.clone(),
                    },
                    &[token],
                )
                .map_err(|error| error.message)?;
            context.close_receiver(receiver)?;
            self.started = true;
        }
        match context.join()? {
            Poll::Pending => Ok(Poll::Pending),
            Poll::Ready(()) => Ok(Poll::Ready(self.child_return.load(Ordering::SeqCst))),
        }
    }
}

#[test]
fn generated_send_leaf_returns_the_original_message_when_closed() {
    let program = core::lower(&phoebe_syntax::syntax::parse(PRODUCER_SOURCE).unwrap()).unwrap();
    let executable = compile_poll_leaf_jit(&program, "producer").unwrap();
    let host = ClosedSendHost {
        entry: unsafe { executable.entry() },
        frame_words: executable.layout().total_words,
        sender_word: executable.layout().entry_params[0].words.start,
        child_return: Arc::new(AtomicI64::new(-1)),
        started: false,
    };
    let mut scheduler = Scheduler::new(host, 2, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap(), 20);
    scheduler.verify().unwrap();
}
