use phoebe::runtime::ScopeLimits;
use phoebe::{core, native, syntax};
use std::process::Command;

#[test]
fn capability_sums_agree_in_interpreter_jit_and_standalone() {
    for (index, (source, expected)) in [
        (include_str!("../examples/capability_option.phoebe"), 3),
        (include_str!("../examples/capability_result.phoebe"), 49),
        (include_str!("../examples/capability_sum_close.phoebe"), 54),
        (
            include_str!("../examples/capability_sum_cleanup.phoebe"),
            15,
        ),
        (include_str!("../examples/capability_sum_product.phoebe"), 3),
        (include_str!("../examples/capability_nested_sum.phoebe"), 84),
    ]
    .into_iter()
    .enumerate()
    {
        let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
        core::verify(&program).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), expected);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), expected);
        let path = std::env::temp_dir().join(format!(
            "phoebe-capability-sum-{}-{index}",
            std::process::id()
        ));
        native::build_executable(&program, &path).unwrap();
        let output = Command::new(&path).output().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            expected.to_string()
        );
    }
}

#[test]
fn early_error_closes_the_owned_sum_envelope() {
    let source = include_str!("../examples/capability_sum_cleanup.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    let acquire = &program.functions[0];
    let failures = acquire
        .blocks
        .iter()
        .filter_map(|block| match block.terminator {
            core::Terminator::ChannelNew { failure, .. } => Some(failure),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 2);
    assert_eq!(
        acquire.blocks[failures[1].0]
            .instructions
            .iter()
            .filter(|instruction| matches!(instruction.operation, core::Operation::CloseCaps(_)))
            .count(),
        1
    );
}

#[test]
fn sum_rights_cannot_be_copied() {
    let copy = "fn bad(maybe: Option<Sender<I64>>) -> I64 { val other = copy(maybe); maybe.close_caps(); other.close_caps(); 0 } fn main() -> I64 = 0";
    let error = core::lower(&syntax::parse(copy).unwrap()).unwrap_err();
    assert!(error.message.contains("copy cannot duplicate"), "{error:?}");
    assert!(error.span.start > 0);
}

#[test]
fn tagged_spawn_transfers_only_active_rights_across_all_engines() {
    let source = r#"
data Pair { sender: Sender<I64>, receiver: Receiver<I64> }
data Envelope { maybe: Option<Pair>, extra: Sender<I64> }
data Five { a: Sender<I64>, b: Sender<I64>, c: Sender<I64>, d: Sender<I64>, e: Sender<I64> }

fn close_envelope(value: Envelope) -> Unit { value.close_caps(); }
fn close_five(value: Option<Five>) -> Unit { yield; value.close_caps(); }

fn direct() -> Result<I64, ResourceError> {
  val (a, b) = channel<I64>(capacity = 1)?;
  val (extra, extra_receiver) = channel<I64>(capacity = 1)?;
  extra_receiver.close();
  val full = Envelope(Option<Pair>.Some(Pair(a, b)), extra);
  spawn close_envelope(full)?;
  val (bare, bare_receiver) = channel<I64>(capacity = 1)?;
  bare_receiver.close();
  val empty = Envelope(Option<Pair>.None, bare);
  spawn close_envelope(empty)?;
  Result<I64, ResourceError>.Ok(20)
}

fn retained() -> Result<I64, ResourceError> {
  yield;
  val (a, ar) = channel<I64>(capacity = 1)?;
  val (b, br) = channel<I64>(capacity = 1)?;
  val (c, cr) = channel<I64>(capacity = 1)?;
  val (d, dr) = channel<I64>(capacity = 1)?;
  val (e, er) = channel<I64>(capacity = 1)?;
  ar.close(); br.close(); cr.close(); dr.close(); er.close();
  val five = Option<Five>.Some(Five(a, b, c, d, e));
  spawn close_five(five)?;
  val none = Option<Five>.None;
  spawn close_five(none)?;
  Result<I64, ResourceError>.Ok(22)
}

fn value(result: Result<I64, ResourceError>) -> I64 = when (result) {
  Result<I64, ResourceError>.Ok(number) -> { number },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}
fn main() -> I64 = value(direct()) + value(retained())
"#;
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    core::verify(&program).unwrap();
    let suspendable = phoebe::frame_layout::suspendable_functions(&program).unwrap();
    assert!(!suspendable[2], "direct parent should use direct codegen");
    assert!(suspendable[3], "retained parent should use poll codegen");
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let limited = ScopeLimits {
        tasks: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program.interpret_with_limits("main", &[], limited).unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], limited).unwrap(),
        0
    );
    let path = std::env::temp_dir().join(format!("phoebe-tagged-spawn-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn core_rejects_discarding_a_capability_bearing_match_payload() {
    let span = syntax::Span { start: 0, end: 0 };
    let program = core::Program {
        data: vec![core::DataDef {
            name: "MaybeSender".into(),
            variants: vec!["None".into(), "Some".into()],
            payloads: vec![None, Some(core::Type::SenderI64)],
            fields: vec![],
        }],
        functions: vec![core::Function {
            name: "bad".into(),
            param_modes: vec![syntax::syntax::ParamMode::Own],
            return_type: core::Type::I64,
            blocks: vec![
                core::Block {
                    params: vec![core::ValueId(0)],
                    instructions: vec![],
                    terminator: core::Terminator::Match {
                        subject: core::ValueId(0),
                        arms: vec![
                            core::MatchArm {
                                target: core::BlockId(1),
                                binding: core::MatchBinding::Discard,
                            },
                            core::MatchArm {
                                target: core::BlockId(2),
                                binding: core::MatchBinding::Discard,
                            },
                        ],
                    },
                },
                core::Block {
                    params: vec![],
                    instructions: vec![core::Instruction {
                        result: Some(core::ValueId(1)),
                        operation: core::Operation::Const(0),
                        span,
                    }],
                    terminator: core::Terminator::Return(core::ValueId(1)),
                },
                core::Block {
                    params: vec![],
                    instructions: vec![core::Instruction {
                        result: Some(core::ValueId(2)),
                        operation: core::Operation::Const(1),
                        span,
                    }],
                    terminator: core::Terminator::Return(core::ValueId(2)),
                },
            ],
            value_types: vec![
                core::Type::Data(core::DataId(0)),
                core::Type::I64,
                core::Type::I64,
            ],
            place_types: vec![],
            loan_regions: vec![],
        }],
    };
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("match payload parameter mismatch")
    );
}
