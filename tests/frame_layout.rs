use phoebe::core::{FunctionId, Operation, PlaceId, ValueId};
use phoebe::{core, frame_layout, syntax};

#[test]
fn only_suspendable_functions_receive_frames() {
    let source = include_str!("../examples/channel_exchange.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let suspendable = frame_layout::suspendable_functions(&program).unwrap();
    assert_eq!(suspendable, vec![true, true, true]);
    let frames = frame_layout::analyze(&program).unwrap();
    assert_eq!(frames.len(), 3);
    assert!(
        frames
            .iter()
            .any(|frame| frame.function == FunctionId(0) && frame.points.len() == 2)
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.function == FunctionId(1) && frame.points.len() == 2)
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.function == FunctionId(2) && frame.points.len() == 1)
    );

    let source = include_str!("../examples/spawn_close.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(
        frame_layout::suspendable_functions(&program).unwrap(),
        vec![false; 3]
    );
    assert!(frame_layout::analyze(&program).unwrap().is_empty());
}

#[test]
fn suspension_spills_only_future_values_and_live_places() {
    let source = r#"
fn wait(receiver: Receiver<I64>) -> I64 {
  val dead = 999;
  var count = 5;
  val item = receiver.receive();
  receiver.close();
  when (item) {
    Receive<I64>.Value(value) -> { count + value },
    Receive<I64>.End -> { count }
  }
}

fn wrapper(receiver: Receiver<I64>) -> I64 {
  val marker = 7;
  val answer = wait(receiver);
  marker + answer
}

fn main() -> I64 = 0
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let frames = frame_layout::analyze(&program).unwrap();
    assert_eq!(frames.len(), 2);

    let wait = &frames[0];
    assert_eq!(wait.function, FunctionId(0));
    assert_eq!(wait.points.len(), 1);
    assert_eq!(wait.points[0].values, vec![ValueId(0)]);
    assert_eq!(wait.points[0].places, vec![PlaceId(0)]);
    assert_eq!(wait.value_slots, vec![ValueId(0)]);

    let wrapper = &frames[1];
    assert_eq!(wrapper.function, FunctionId(1));
    assert_eq!(wrapper.points.len(), 1);
    assert_eq!(wrapper.points[0].instruction, Some(1));
    let marker = program.functions[1].blocks[0]
        .instructions
        .iter()
        .find_map(|instruction| match instruction.operation {
            Operation::Const(7) => instruction.result,
            _ => None,
        })
        .unwrap();
    assert_eq!(wrapper.value_slots, vec![marker]);
    assert!(wrapper.place_slots.is_empty());
}

#[test]
fn loop_backedge_keeps_mutable_place_and_receiver_across_retry() {
    let source = r#"
fn wait(receiver: Receiver<I64>) -> I64 {
  var count = 0;
  while (count < 2) {
    val next = receiver.receive();
    count = count + 1;
    when (next) {
      Receive<I64>.Value(value) -> { value },
      Receive<I64>.End -> { 0 }
    };
  }
  receiver.close();
  count
}

fn main() -> I64 = 0
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let frames = frame_layout::analyze(&program).unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].points.len(), 1);
    assert!(frames[0].points[0].values.contains(&ValueId(0)));
    assert_eq!(frames[0].points[0].places, vec![PlaceId(0)]);
}

#[test]
fn pending_borrowed_call_retains_the_caller_place() {
    let source = r#"
fn wait(value: &I64, receiver: Receiver<I64>) -> I64 {
  val item = receiver.receive();
  receiver.close();
  value
}

fn wrapper(receiver: Receiver<I64>) -> I64 {
  var held = 7;
  wait(&held, receiver)
}

fn main() -> I64 = 0
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let frames = frame_layout::analyze(&program).unwrap();
    assert_eq!(frames.len(), 2);
    assert!(frames[0].points[0].values.contains(&ValueId(0)));
    assert_eq!(frames[1].points[0].places, vec![PlaceId(0)]);
}
