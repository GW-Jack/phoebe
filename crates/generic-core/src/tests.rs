use crate::{Block, BlockId, Function, Instruction, Operation, Terminator, Type, ValueId, verify};

fn identity() -> Function {
    Function {
        name: "identity".into(),
        type_params: 1,
        params: vec![Type::Param(0)],
        result: Type::Param(0),
        value_types: vec![Type::Param(0)],
        blocks: vec![Block {
            params: vec![ValueId(0)],
            instructions: Vec::new(),
            terminator: Terminator::Return(ValueId(0)),
        }],
    }
}

#[test]
fn opaque_identity_is_checked_without_a_concrete_abi_shape() {
    verify(&identity()).unwrap();
    let mut moved = identity();
    moved.value_types.push(Type::Param(0));
    moved.blocks[0].instructions.push(Instruction {
        result: ValueId(1),
        operation: Operation::Move(ValueId(0)),
    });
    moved.blocks[0].terminator = Terminator::Return(ValueId(1));
    verify(&moved).unwrap();
    moved.blocks[0].terminator = Terminator::Return(ValueId(0));
    assert!(verify(&moved).unwrap_err().contains("not live"));
}

#[test]
fn branch_join_forwards_one_opaque_owner_on_each_path() {
    let function = Function {
        name: "choose".into(),
        type_params: 1,
        params: vec![Type::Param(0), Type::Bool],
        result: Type::Param(0),
        value_types: vec![
            Type::Param(0),
            Type::Bool,
            Type::Param(0),
            Type::Param(0),
            Type::Param(0),
        ],
        blocks: vec![
            Block {
                params: vec![ValueId(0), ValueId(1)],
                instructions: Vec::new(),
                terminator: Terminator::Branch {
                    condition: ValueId(1),
                    when_true: BlockId(1),
                    true_args: vec![ValueId(0)],
                    when_false: BlockId(2),
                    false_args: vec![ValueId(0)],
                },
            },
            Block {
                params: vec![ValueId(2)],
                instructions: Vec::new(),
                terminator: Terminator::Jump {
                    target: BlockId(3),
                    args: vec![ValueId(2)],
                },
            },
            Block {
                params: vec![ValueId(3)],
                instructions: Vec::new(),
                terminator: Terminator::Jump {
                    target: BlockId(3),
                    args: vec![ValueId(3)],
                },
            },
            Block {
                params: vec![ValueId(4)],
                instructions: Vec::new(),
                terminator: Terminator::Return(ValueId(4)),
            },
        ],
    };
    verify(&function).unwrap();
    let mut invalid = function;
    invalid.blocks[2].terminator = Terminator::Jump {
        target: BlockId(3),
        args: vec![ValueId(2)],
    };
    assert!(
        verify(&invalid)
            .unwrap_err()
            .contains("not defined in this block")
    );
}

#[test]
fn opaque_owner_cannot_be_copied_lost_or_sent_twice() {
    let mut copied = identity();
    copied.value_types.push(Type::Param(0));
    copied.blocks[0].instructions.push(Instruction {
        result: ValueId(1),
        operation: Operation::CopyScalar(ValueId(0)),
    });
    copied.blocks[0].terminator = Terminator::Return(ValueId(1));
    assert!(verify(&copied).unwrap_err().contains("cannot be copied"));

    let leaked = Function {
        name: "leaked".into(),
        type_params: 1,
        params: vec![Type::Param(0), Type::I64],
        result: Type::I64,
        value_types: vec![Type::Param(0), Type::I64],
        blocks: vec![Block {
            params: vec![ValueId(0), ValueId(1)],
            instructions: Vec::new(),
            terminator: Terminator::Return(ValueId(1)),
        }],
    };
    assert!(verify(&leaked).unwrap_err().contains("normal return leaks"));

    let duplicate = Function {
        name: "duplicate".into(),
        type_params: 1,
        params: vec![Type::Param(0)],
        result: Type::Param(0),
        value_types: vec![Type::Param(0); 3],
        blocks: vec![
            Block {
                params: vec![ValueId(0)],
                instructions: Vec::new(),
                terminator: Terminator::Jump {
                    target: BlockId(1),
                    args: vec![ValueId(0), ValueId(0)],
                },
            },
            Block {
                params: vec![ValueId(1), ValueId(2)],
                instructions: Vec::new(),
                terminator: Terminator::Return(ValueId(1)),
            },
        ],
    };
    assert!(verify(&duplicate).unwrap_err().contains("moved twice"));
}

#[test]
fn malformed_type_binder_and_unreachable_block_are_rejected() {
    let mut unbound = identity();
    unbound.result = Type::Param(1);
    assert!(
        verify(&unbound)
            .unwrap_err()
            .contains("unbound type parameter")
    );

    let mut unreachable = identity();
    unreachable.value_types.push(Type::Param(0));
    unreachable.blocks.push(Block {
        params: vec![ValueId(1)],
        instructions: Vec::new(),
        terminator: Terminator::Return(ValueId(1)),
    });
    assert!(
        verify(&unreachable)
            .unwrap_err()
            .contains("unreachable block")
    );
}

#[test]
fn source_identity_and_branch_lower_to_checked_parametric_core() {
    let source = r#"
fn identity<T>(x: T) -> T = x
fn choose<T>(flag: Bool, value: T) -> T = if (flag) { value } else { value }
fn forwarded<T>(value: T) -> T {
  val next = value;
  next
}
"#;
    let ast = phoebe_syntax::parse(source).unwrap();
    let shifted = phoebe_syntax::parse(&format!("\n{source}")).unwrap();
    assert!(phoebe_syntax::same_function_with_shift(
        &ast.functions[0],
        &shifted.functions[0],
        1,
    ));
    assert_eq!(ast.functions.len(), 3);
    for function in &ast.functions {
        let lowered = crate::lower(function).unwrap();
        verify(&lowered).unwrap();
        assert_eq!(lowered.type_params, 1);
        assert!(lowered.to_string().contains("-> T0 {"));
    }
    assert_eq!(crate::lower(&ast.functions[1]).unwrap().blocks.len(), 3);
    assert!(
        crate::lower(&ast.functions[1])
            .unwrap()
            .to_string()
            .contains("branch v0 ? b1(v1) : b2(v1)")
    );
}

#[test]
fn source_generic_copy_and_unsupported_operations_get_spans() {
    for (source, message) in [
        (
            "fn bad<T>(x: T) -> T { val y = copy(x); y }",
            "cannot be copied",
        ),
        ("fn bad<T>(x: T) -> T = x.sender", "not implemented yet"),
        ("fn bad<T>(x: T) -> T { val y = x; x }", "not live"),
    ] {
        let ast = phoebe_syntax::parse(source).unwrap();
        let error = crate::lower(&ast.functions[0]).unwrap_err();
        assert!(error.message.contains(message), "{error:?}");
        assert!(error.span.end > error.span.start, "{error:?}");
    }
}

#[test]
fn generic_calls_infer_binders_and_transfer_opaque_owners() {
    let source = r#"
fn identity<T>(x: T) -> T = x
fn through<U>(value: U) -> U = identity(value)
fn through_val<U>(value: U) -> U {
  val next = identity(value);
  next
}
"#;
    let ast = phoebe_syntax::parse(source).unwrap();
    let module = crate::lower_program(&ast).unwrap();
    crate::verify_module(&module).unwrap();
    assert_eq!(module.functions.len(), 3);
    assert!(module.to_string().contains("call f0<T0>(v0)"));
    assert!(matches!(
        module.functions[1].blocks[0].instructions[0].operation,
        Operation::Call { callee: 0, .. }
    ));

    let mut wrong_result = module.clone();
    wrong_result.functions[1].value_types[1] = Type::I64;
    assert!(
        crate::verify_module(&wrong_result)
            .unwrap_err()
            .contains("result type mismatch")
    );
}

#[test]
fn generic_call_rejects_mismatched_and_reused_owners() {
    let source = r#"
fn first<T>(left: T, right: T) -> T = left
fn bad<U>(left: U, right: I64) -> U = first(left, right)
"#;
    let ast = phoebe_syntax::parse(source).unwrap();
    assert!(
        crate::lower_program(&ast)
            .unwrap_err()
            .message
            .contains("mismatched arguments")
    );

    let source = r#"
fn identity<T>(x: T) -> T = x
fn bad<U>(x: U) -> U {
  val y = identity(x);
  x
}
"#;
    let ast = phoebe_syntax::parse(source).unwrap();
    let error = crate::lower_program(&ast).unwrap_err();
    assert!(error.message.contains("not live"));
    assert_eq!(error.span, ast.functions[1].name_span);

    let source = r#"
fn identity<T>(x: T) -> T = x
fn bad<U>(x: U) -> U {
  val y = identity(x);
  val z = identity(x);
  y
}
"#;
    let ast = phoebe_syntax::parse(source).unwrap();
    assert!(
        crate::lower_program(&ast)
            .unwrap_err()
            .message
            .contains("not live")
    );
}
