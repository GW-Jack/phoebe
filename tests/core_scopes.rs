use phoebe::Span;
use phoebe::core::{
    self, Block, BlockId, Function, FunctionId, Instruction, Operation, PlaceId, Program,
    TaskScopeId, Terminator, Type, ValueId,
};
use phoebe::{native, runtime};

const SPAN: Span = Span { start: 5, end: 10 };

fn instruction(result: Option<usize>, operation: Operation) -> Instruction {
    Instruction {
        result: result.map(ValueId),
        operation,
        span: SPAN,
    }
}

fn scoped_spawn() -> Program {
    Program {
        data: vec![],
        functions: vec![
            Function {
                name: "main".into(),
                param_modes: vec![],
                return_type: Type::I64,
                value_types: vec![Type::I64, Type::ResourceError, Type::I64],
                place_types: vec![],
                loan_regions: vec![],
                blocks: vec![
                    Block {
                        params: vec![],
                        instructions: vec![
                            instruction(None, Operation::ScopeEnter(TaskScopeId(0))),
                            instruction(Some(0), Operation::Const(42)),
                        ],
                        terminator: Terminator::Spawn {
                            child: FunctionId(1),
                            args: vec![],
                            success: BlockId(1),
                            failure: BlockId(2),
                            span: SPAN,
                        },
                    },
                    Block {
                        params: vec![],
                        instructions: vec![],
                        terminator: Terminator::ScopeJoin {
                            scope: TaskScopeId(0),
                            resume: BlockId(3),
                            span: SPAN,
                        },
                    },
                    Block {
                        params: vec![ValueId(1)],
                        instructions: vec![instruction(Some(2), Operation::Const(0))],
                        terminator: Terminator::ScopeJoin {
                            scope: TaskScopeId(0),
                            resume: BlockId(4),
                            span: SPAN,
                        },
                    },
                    Block {
                        params: vec![],
                        instructions: vec![],
                        terminator: Terminator::Return(ValueId(0)),
                    },
                    Block {
                        params: vec![],
                        instructions: vec![],
                        terminator: Terminator::Return(ValueId(2)),
                    },
                ],
            },
            Function {
                name: "child".into(),
                param_modes: vec![],
                return_type: Type::I64,
                value_types: vec![Type::I64],
                place_types: vec![],
                loan_regions: vec![],
                blocks: vec![Block {
                    params: vec![],
                    instructions: vec![instruction(Some(0), Operation::Const(7))],
                    terminator: Terminator::Return(ValueId(0)),
                }],
            },
        ],
    }
}

#[test]
fn scope_join_parks_in_interpreter_and_native_code() {
    let program = scoped_spawn();
    core::verify(&program).unwrap();
    assert!(program.to_string().contains("scope.enter r0"));
    assert!(program.to_string().contains("scope.join r0"));
    let events = program.cost_events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "scope.enter")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "scope.join")
            .count(),
        2
    );
    assert!(events.iter().all(|event| event.span == SPAN));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let path = std::env::temp_dir().join(format!("phoebe-core-scope-{}", std::process::id()));
    native::build_executable(&program, &path).unwrap();
    let output = std::process::Command::new(&path).output().unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
    let limits = runtime::ScopeLimits {
        tasks: 1,
        channels: 0,
        replies: 0,
        ..runtime::ScopeLimits::default()
    };
    assert_eq!(
        program.interpret_with_limits("main", &[], limits).unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], limits).unwrap(),
        0
    );
}

#[test]
fn verifier_rejects_unmatched_and_unclosed_scope_regions() {
    let mut program = scoped_spawn();
    program.functions[0].blocks[0].instructions.remove(0);
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("not the innermost")
    );

    let mut program = scoped_spawn();
    program.functions[0].blocks[3]
        .instructions
        .push(instruction(None, Operation::ScopeEnter(TaskScopeId(1))));
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("leaves a task scope open")
    );

    let mut program = scoped_spawn();
    program.functions[0].blocks[0]
        .instructions
        .insert(0, instruction(None, Operation::ScopeEnter(TaskScopeId(0))));
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("duplicate scope region")
    );
}

#[test]
fn scope_join_retains_a_mutable_place_across_pending_child() {
    let mut program = scoped_spawn();
    let main = &mut program.functions[0];
    main.place_types.push(Type::I64);
    main.value_types.push(Type::I64);
    main.blocks[0].instructions.push(instruction(
        None,
        Operation::PlaceInit(PlaceId(0), ValueId(0)),
    ));
    main.blocks[3]
        .instructions
        .push(instruction(Some(3), Operation::Load(PlaceId(0))));
    main.blocks[3].terminator = Terminator::Return(ValueId(3));
    core::verify(&program).unwrap();
    let frames = phoebe::frame_layout::analyze(&program).unwrap();
    assert!(
        frames[0]
            .points
            .iter()
            .any(|point| point.places.contains(&PlaceId(0)))
    );
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn verifier_rejects_a_branch_that_skips_scope_join() {
    let program = Program {
        data: vec![],
        functions: vec![Function {
            name: "main".into(),
            param_modes: vec![],
            return_type: Type::I64,
            value_types: vec![Type::Bool, Type::I64],
            place_types: vec![],
            loan_regions: vec![],
            blocks: vec![
                Block {
                    params: vec![],
                    instructions: vec![
                        instruction(None, Operation::ScopeEnter(TaskScopeId(0))),
                        instruction(Some(0), Operation::Bool(true)),
                        instruction(Some(1), Operation::Const(7)),
                    ],
                    terminator: Terminator::Branch {
                        condition: ValueId(0),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    },
                },
                Block {
                    params: vec![],
                    instructions: vec![],
                    terminator: Terminator::ScopeJoin {
                        scope: TaskScopeId(0),
                        resume: BlockId(3),
                        span: SPAN,
                    },
                },
                Block {
                    params: vec![],
                    instructions: vec![],
                    terminator: Terminator::Jump(BlockId(3), vec![]),
                },
                Block {
                    params: vec![],
                    instructions: vec![],
                    terminator: Terminator::Return(ValueId(1)),
                },
            ],
        }],
    };
    assert!(
        core::verify(&program)
            .unwrap_err()
            .contains("merges different open task scopes")
    );
}
