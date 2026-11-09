use phoebe::runtime::{NativePollTask, Scheduler};
use phoebe::{core, native, syntax};
use std::process::Command;

const PAIR: &str = "data Pair { a: I64, b: I64 }\n";

fn lower(source: &str) -> core::Program {
    core::lower(&syntax::parse(source).unwrap()).unwrap()
}

#[test]
fn destination_selection_agrees_across_execution_paths() {
    let cases = [
        (
            "fn main() -> I64 { val pair = Pair(20, 22); pair.a + pair.b }",
            42,
        ),
        (
            "data Marker { enabled: Bool, spare: Unit }\n\
             fn main() -> I64 { val marker = Marker(true, Unit); if (marker.enabled) { 42 } else { 0 } }",
            42,
        ),
        (
            "fn second() -> Result<I64, I64> = Result<I64, I64>.Err(7)\n\
             fn build() -> Result<Pair, I64> { val pair = Pair(20, second()?); Result<Pair, I64>.Ok(pair) }\n\
             fn main() -> I64 = when (build()) {\n\
               Result<Pair, I64>.Ok(pair) -> { pair.a + pair.b },\n\
               Result<Pair, I64>.Err(error) -> { error },\n\
             }",
            7,
        ),
        (
            "fn second() -> Result<I64, I64> { yield; Result<I64, I64>.Ok(22) }\n\
             fn build() -> Result<Pair, I64> { val pair = Pair(20, second()?); Result<Pair, I64>.Ok(pair) }\n\
             fn main() -> I64 = when (build()) {\n\
               Result<Pair, I64>.Ok(pair) -> { pair.a + pair.b },\n\
               Result<Pair, I64>.Err(error) -> { error },\n\
             }",
            42,
        ),
    ];
    for (index, (body, expected)) in cases.into_iter().enumerate() {
        let program = lower(&format!("{PAIR}{body}"));
        let ir = program.to_string();
        if index < 2 {
            assert!(!ir.contains("dest.begin"), "{ir}");
            assert!(ir.contains("product"), "{ir}");
        } else {
            assert!(ir.contains("dest.begin"), "{ir}");
            assert!(ir.contains("dest.publish"), "{ir}");
        }
        if index == 0 {
            assert!(program.cost_events().is_empty());
            let clif = native::clif(&program).unwrap();
            assert!(!clif.contains("stack_slot"), "{clif}");
        }
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path =
            std::env::temp_dir().join(format!("phoebe-destination-{}-{index}", std::process::id()));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
    }
}

#[test]
fn verifier_checks_destination_field_completion() {
    let mut program = lower(&format!(
        "{PAIR}fn main() -> I64 {{ val pair = Pair(20, 22); pair.a + pair.b }}"
    ));
    let function = &mut program.functions[0];
    let product_index = function.blocks[0]
        .instructions
        .iter()
        .position(|item| matches!(item.operation, core::Operation::Product(_, _)))
        .unwrap();
    let product = function.blocks[0].instructions.remove(product_index);
    let core::Operation::Product(_, fields) = product.operation else {
        unreachable!()
    };
    let place = core::PlaceId(function.place_types.len());
    function.place_types.push(core::Type::Data(core::DataId(0)));
    function.blocks[0].instructions.splice(
        product_index..product_index,
        [
            core::Instruction {
                result: None,
                operation: core::Operation::DestBegin(place),
                span: product.span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::DestField(place, 0, fields[0]),
                span: product.span,
            },
            core::Instruction {
                result: None,
                operation: core::Operation::DestField(place, 1, fields[1]),
                span: product.span,
            },
            core::Instruction {
                result: product.result,
                operation: core::Operation::DestPublish(place),
                span: product.span,
            },
        ],
    );
    core::verify(&program).unwrap();
    let instructions = &program.functions[0].blocks[0].instructions;
    let begin = instructions
        .iter()
        .position(|item| matches!(item.operation, core::Operation::DestBegin(_)))
        .unwrap();
    let second_field = instructions
        .iter()
        .position(|item| matches!(item.operation, core::Operation::DestField(_, 1, _)))
        .unwrap();
    let publication = instructions
        .iter()
        .position(|item| matches!(item.operation, core::Operation::DestPublish(_)))
        .unwrap();

    let mut missing = program.clone();
    missing.functions[0].blocks[0]
        .instructions
        .remove(second_field);
    assert!(
        core::verify(&missing)
            .unwrap_err()
            .contains("field was not initialized")
    );

    let mut duplicate = program.clone();
    if let core::Operation::DestField(_, index, _) =
        &mut duplicate.functions[0].blocks[0].instructions[second_field].operation
    {
        *index = 0;
    }
    assert!(
        core::verify(&duplicate)
            .unwrap_err()
            .contains("duplicate destination field")
    );

    let mut premature = program.clone();
    premature.functions[0].blocks[0]
        .instructions
        .swap(second_field, publication);
    assert!(
        core::verify(&premature)
            .unwrap_err()
            .contains("published before field initialization")
    );

    let mut missing_begin = program;
    missing_begin.functions[0].blocks[0]
        .instructions
        .remove(begin);
    assert!(core::verify(&missing_begin).is_err());
}

#[test]
fn owned_destination_closes_only_initialized_rights_on_early_error() {
    for capacity in [0, 1] {
        let source = format!(
            r#"
data Envelope {{ sender: Sender<I64>, number: I64 }}
fn second() -> Result<I64, ResourceError> {{
  val (sender, receiver) = channel<I64>(capacity = {capacity})?;
  sender.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(22)
}}
fn build() -> Result<I64, ResourceError> {{
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val envelope = Envelope(sender, second()?);
  receiver.close();
  envelope.close_caps();
  Result<I64, ResourceError>.Ok(42)
}}
fn main() -> I64 = when (build()) {{
  Result<I64, ResourceError>.Ok(value) -> {{ value }},
  Result<I64, ResourceError>.Err(error) -> {{ 42 }},
}}
"#
        );
        let program = lower(&source);
        let ir = program.to_string();
        assert!(ir.contains("dest.take.field"), "{ir}");
        assert!(ir.contains("dest.publish"), "{ir}");
        let failure_source = source.find("second()?").unwrap();
        assert!(program.cost_events().iter().any(|event| {
            event.function == "build"
                && event.kind == "channel.op"
                && event.reason == "owned capability close"
                && event.span.start == failure_source
        }));
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
        let path = std::env::temp_dir().join(format!(
            "phoebe-owned-destination-{}-{capacity}",
            std::process::id()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"42\n");
        if capacity == 0 {
            let mut duplicate = program.clone();
            let function = duplicate
                .functions
                .iter_mut()
                .find(|function| function.name == "build")
                .unwrap();
            let block = function
                .blocks
                .iter_mut()
                .find(|block| {
                    block.instructions.iter().any(|instruction| {
                        matches!(instruction.operation, core::Operation::DestTakeField(_, _))
                    })
                })
                .unwrap();
            let index = block
                .instructions
                .iter()
                .position(|instruction| {
                    matches!(instruction.operation, core::Operation::DestTakeField(_, _))
                })
                .unwrap();
            let result = core::ValueId(function.value_types.len());
            function.value_types.push(core::Type::SenderI64);
            let mut second_take = block.instructions[index].clone();
            second_take.result = Some(result);
            block.instructions.insert(index + 1, second_take.clone());
            block.instructions.insert(
                index + 2,
                core::Instruction {
                    result: None,
                    operation: core::Operation::CloseHandle(result),
                    span: second_take.span,
                },
            );
            let error = core::verify(&duplicate).unwrap_err();
            assert!(error.contains("not live"), "{error}");
        }
    }
}

#[test]
fn nested_try_in_second_field_cleans_owned_first_field() {
    for (second, expected) in [
        ("yield; Result<I64, I64>.Ok(22)", 23),
        ("Result<I64, I64>.Err(7)", 42),
    ] {
        let source = format!(
            r#"
data Envelope {{ sender: Sender<I64>, number: I64 }}
fn second() -> Result<I64, I64> {{ {second} }}
fn build(sender: Sender<I64>) -> Result<I64, I64> {{
  val envelope = Envelope(sender, second()? + 1);
  val number = envelope.number;
  envelope.close_caps();
  Result<I64, I64>.Ok(number)
}}
fn entry() -> Result<I64, ResourceError> {{
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  receiver.close();
  val result = build(sender);
  val value = when (result) {{
    Result<I64, I64>.Ok(number) -> {{ number }},
    Result<I64, I64>.Err(error) -> {{ 42 }},
  }};
  Result<I64, ResourceError>.Ok(value)
}}
fn main() -> I64 = when (entry()) {{
  Result<I64, ResourceError>.Ok(value) -> {{ value }},
  Result<I64, ResourceError>.Err(error) -> {{ 42 }},
}}
"#
        );
        let program = lower(&source);
        let ir = program.to_string();
        assert!(ir.contains("dest.take.field"), "{ir}");
        assert!(ir.contains("dest.publish"), "{ir}");
        let failure_source = source.find("second()?").unwrap();
        assert!(program.cost_events().iter().any(|event| {
            event.function == "build"
                && event.kind == "channel.op"
                && event.reason == "owned capability close"
                && event.span.start == failure_source
        }));
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path = std::env::temp_dir().join(format!(
            "phoebe-nested-try-destination-{}-{expected}",
            std::process::id()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
    }
}

#[test]
fn three_field_destination_cleans_each_initialized_right() {
    let source = r#"
data Envelope { first: Sender<I64>, second: Sender<I64>, number: I64 }
fn number() -> Result<I64, I64> { yield; Result<I64, I64>.Err(7) }
fn build(first: Sender<I64>, second: Sender<I64>) -> Result<I64, I64> {
  val envelope = Envelope(first, second, number()? + 1);
  val number = envelope.number;
  envelope.close_caps();
  Result<I64, I64>.Ok(number)
}

fn entry() -> Result<I64, ResourceError> {
  val (first, first_receiver) = channel<I64>(capacity = 1)?;
  first_receiver.close();
  val (second, second_receiver) = channel<I64>(capacity = 1)?;
  second_receiver.close();
  val result = build(first, second);
  val value = when (result) {
    Result<I64, I64>.Ok(number) -> { number },
    Result<I64, I64>.Err(error) -> { 42 },
  };
  Result<I64, ResourceError>.Ok(value)
}
fn main() -> I64 = when (entry()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = lower(source);
    let ir = program.to_string();
    assert_eq!(ir.matches("dest.take.field").count(), 2, "{ir}");
    assert!(ir.contains("dest.publish"), "{ir}");
    let mut missing = program.clone();
    let build = missing
        .functions
        .iter_mut()
        .find(|function| function.name == "build")
        .unwrap();
    let block = build
        .blocks
        .iter_mut()
        .find(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(instruction.operation, core::Operation::DestField(_, 2, _))
            })
        })
        .unwrap();
    block.instructions.retain(|instruction| {
        !matches!(instruction.operation, core::Operation::DestField(_, 2, _))
    });
    assert!(
        core::verify(&missing)
            .unwrap_err()
            .contains("destination field was not initialized")
    );
    let failure_source = source.find("number()?").unwrap();
    assert_eq!(
        program
            .cost_events()
            .iter()
            .filter(|event| {
                event.function == "build"
                    && event.kind == "channel.op"
                    && event.reason == "owned capability close"
                    && event.span.start == failure_source
            })
            .count(),
        2
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!(
        "phoebe-three-field-destination-{}",
        std::process::id()
    ));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn active_variant_destination_publishes_only_its_payload() {
    for (second, expected) in [
        ("yield; Result<I64, I64>.Ok(42)", 42),
        ("Result<I64, I64>.Err(7)", 7),
    ] {
        let source = format!(
            "fn second() -> Result<I64, I64> {{ {second} }}\n\
             fn build() -> Result<I64, I64> = Result<I64, I64>.Ok(second()?)\n\
             fn main() -> I64 = when (build()) {{\n\
               Result<I64, I64>.Ok(value) -> {{ value }},\n\
               Result<I64, I64>.Err(error) -> {{ error }},\n\
             }}"
        );
        let program = lower(&source);
        let ir = program.to_string();
        assert!(ir.contains("dest.begin.variant"), "{ir}");
        assert!(ir.contains("dest.publish"), "{ir}");
        for kind in ["destination.zero", "destination.write"] {
            assert!(program.cost_events().iter().any(|event| {
                event.function == "build" && event.kind == kind && event.span.end > event.span.start
            }));
        }
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path = std::env::temp_dir().join(format!(
            "phoebe-variant-destination-{}-{expected}",
            std::process::id()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
    }
}

#[test]
fn active_variant_destination_transfers_a_right_across_suspension() {
    let source = r#"
fn second() -> Result<Sender<I64>, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  receiver.close();
  yield;
  Result<Sender<I64>, ResourceError>.Ok(sender)
}

fn build() -> Result<Option<Sender<I64>>, ResourceError> =
  Result<Option<Sender<I64>>, ResourceError>.Ok(Option<Sender<I64>>.Some(second()?))
fn main() -> I64 = when (build()) {
  Result<Option<Sender<I64>>, ResourceError>.Ok(value) -> { value.close_caps(); 42 },
  Result<Option<Sender<I64>>, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = lower(source);
    let ir = program.to_string();
    assert!(ir.contains("dest.begin.variant"), "{ir}");
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!(
        "phoebe-right-variant-destination-{}",
        std::process::id()
    ));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn verifier_rejects_incomplete_or_wrong_variant_destinations() {
    let source = "fn second() -> Result<I64, I64> = Result<I64, I64>.Ok(42)\n\
        fn build() -> Result<I64, I64> = Result<I64, I64>.Ok(second()?)\n\
        fn main() -> I64 = when (build()) {\n\
          Result<I64, I64>.Ok(value) -> { value },\n\
          Result<I64, I64>.Err(error) -> { error },\n\
        }";
    let program = lower(source);
    let build_index = program
        .functions
        .iter()
        .position(|function| function.name == "build")
        .unwrap();

    let mut missing = program.clone();
    for block in &mut missing.functions[build_index].blocks {
        block.instructions.retain(|instruction| {
            !matches!(instruction.operation, core::Operation::DestField(_, _, _))
        });
    }
    assert!(
        core::verify(&missing)
            .unwrap_err()
            .contains("destination field was not initialized")
    );

    let mut wrong_tag = program.clone();
    let begin = wrong_tag.functions[build_index]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find(|instruction| {
            matches!(
                instruction.operation,
                core::Operation::DestBeginVariant(_, _)
            )
        })
        .unwrap();
    let core::Operation::DestBeginVariant(_, tag) = &mut begin.operation else {
        unreachable!()
    };
    *tag = 1;
    assert!(
        core::verify(&wrong_tag)
            .unwrap_err()
            .contains("active variant")
    );
}

#[test]
fn nested_owned_destination_closes_both_moved_rights() {
    let source = r#"
data Rights { sender: Sender<I64>, receiver: Receiver<I64> }
data Envelope { rights: Rights, number: I64 }
fn second() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 0)?;
  sender.close();
  receiver.close();
  Result<I64, ResourceError>.Ok(22)
}

fn build() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val rights = Rights(sender, receiver);
  val envelope = Envelope(rights, second()?);
  envelope.close_caps();
  Result<I64, ResourceError>.Ok(42)
}
fn main() -> I64 = when (build()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 42 },
}
"#;
    let program = lower(source);
    let ir = program.to_string();
    assert!(ir.contains("dest.take.field p"), "{ir}");
    assert!(ir.contains("close.caps"), "{ir}");
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!(
        "phoebe-nested-owned-destination-{}",
        std::process::id()
    ));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn pending_owned_destination_is_retained_until_deadlock_cleanup() {
    let source = r#"
data Envelope { sender: Sender<I64>, number: I64 }
fn wait(receiver: Receiver<I64>) -> Result<I64, ResourceError> {
  val ignored = receiver.receive();
  receiver.close();
  Result<I64, ResourceError>.Ok(22)
}

fn build() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val envelope = Envelope(sender, wait(receiver)?);
  envelope.close_caps();
  Result<I64, ResourceError>.Ok(42)
}
fn main() -> I64 = when (build()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = lower(source);
    assert_eq!(
        program.interpret("main", &[]).unwrap_err(),
        "scope deadlocked"
    );
    let executable = native::compile_poll_jit(&program, "main").unwrap();
    let root = unsafe { NativePollTask::new(executable.entry(), executable.layout().total_words) }
        .unwrap();
    let mut scheduler = Scheduler::new(root, 1, 1, 0).unwrap();
    assert_eq!(scheduler.run().unwrap_err(), "scope deadlocked");
    assert_eq!(scheduler.open_handles(), 0);
    scheduler.verify().unwrap();

    let path = std::env::temp_dir().join(format!(
        "phoebe-owned-destination-deadlock-{}",
        std::process::id()
    ));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"phoebe runtime failed: scope deadlocked\n");
}

#[test]
fn wide_product_result_constructs_into_caller_storage() {
    let source = r#"
data Large {
  right: Sender<I64>, a: I64, b: I64, c: I64, d: I64,
  e: I64, f: I64, g: I64, h: I64, i: I64
}
fn make(right: Sender<I64>) -> Large = Large(right, 33, 2, 3, 4, 5, 6, 7, 8, 9)
fn entry() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  receiver.close();
  val large = make(sender);
  val total = large.a + large.i;
  large.close_caps();
  Result<I64, ResourceError>.Ok(total)
}
fn main() -> I64 = when (entry()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
"#;
    let program = lower(source);
    let make = program
        .functions
        .iter()
        .find(|function| function.name == "make")
        .unwrap();
    assert!(matches!(
        make.blocks[0].instructions.last().unwrap().operation,
        core::Operation::DestPublish(_)
    ));
    let clif = native::clif(&program).unwrap();
    let make_clif = clif
        .split("; Phoebe function make\n")
        .nth(1)
        .unwrap()
        .split("; Phoebe function ")
        .next()
        .unwrap();
    assert!(make_clif.contains(" sret)"), "{make_clif}");
    assert!(make_clif.contains("store "), "{make_clif}");
    assert!(!make_clif.contains("explicit_slot"), "{make_clif}");
    assert!(!make_clif.contains("stack_store"), "{make_clif}");
    assert!(!make_clif.contains("stack_load"), "{make_clif}");
    assert_eq!(
        program
            .cost_events()
            .iter()
            .filter(|event| event.function == "make" && event.kind == "destination.write")
            .count(),
        10
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!(
        "phoebe-caller-product-destination-{}",
        std::process::id()
    ));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"42\n");
    let with_call = source.replace(
        "fn make(right: Sender<I64>) -> Large = Large(right, 33, 2, 3, 4, 5, 6, 7, 8, 9)",
        "fn field() -> I64 = 33\nfn make(right: Sender<I64>) -> Large = Large(right, field(), 2, 3, 4, 5, 6, 7, 8, 9)",
    );
    let guarded = lower(&with_call);
    let clif = native::clif(&guarded).unwrap();
    let guarded_make = clif
        .split("; Phoebe function make\n")
        .nth(1)
        .unwrap()
        .split("; Phoebe function ")
        .next()
        .unwrap();
    assert!(guarded_make.contains("explicit_slot"), "{guarded_make}");
    assert_eq!(native::run(&guarded, "main", &[]).unwrap(), 42);
}
