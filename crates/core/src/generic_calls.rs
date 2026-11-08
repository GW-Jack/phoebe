//! Concrete type substitution for a verified parametric call signature.
//! This performs work proportional to the call signature, never the callee
//! body. The linked Core verifier uses it to check a generic call instruction;
//! execution remains a separate backend step.

use phoebe_generic_core::{CheckedModule, Type as GenericType};

use crate::Type;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCall {
    pub callee: usize,
    pub type_args: Vec<Type>,
    pub params: Vec<Type>,
    pub result: Type,
}

fn scalar(ty: GenericType) -> Option<Type> {
    match ty {
        GenericType::Param(_) => None,
        GenericType::I64 => Some(Type::I64),
        GenericType::Bool => Some(Type::Bool),
        GenericType::Unit => Some(Type::Unit),
    }
}

fn substitute(ty: GenericType, type_args: &[Type]) -> Result<Type, String> {
    match ty {
        GenericType::Param(index) => type_args
            .get(index)
            .copied()
            .ok_or_else(|| format!("missing type argument T{index}")),
        concrete => scalar(concrete).ok_or("invalid concrete generic type".into()),
    }
}

fn owning_type(ty: Type) -> Result<(), String> {
    if matches!(
        ty,
        Type::RefI64 | Type::RefMutI64 | Type::RefBool | Type::RefMutBool
    ) {
        Err("borrowed reference cannot instantiate an owned type parameter".into())
    } else {
        Ok(())
    }
}

/// Infer type arguments from owned argument types. A binder that appears only
/// in the result cannot be inferred until explicit source type arguments are
/// available.
pub fn infer(module: &CheckedModule, name: &str, actual: &[Type]) -> Result<ResolvedCall, String> {
    let (callee, function) = module
        .as_module()
        .functions
        .iter()
        .enumerate()
        .find(|(_, function)| function.name == name)
        .ok_or_else(|| format!("unknown generic function {name}"))?;
    if function.params.len() != actual.len() {
        return Err(format!("{name} argument count mismatch"));
    }
    let mut inferred = vec![None; function.type_params];
    for (&formal, &actual) in function.params.iter().zip(actual) {
        match formal {
            GenericType::Param(index) => {
                owning_type(actual)?;
                match &mut inferred[index] {
                    Some(previous) if *previous != actual => {
                        return Err(format!(
                            "{name} repeated type parameter has mismatched arguments"
                        ));
                    }
                    slot => *slot = Some(actual),
                }
            }
            concrete if scalar(concrete) != Some(actual) => {
                return Err(format!("{name} argument type mismatch"));
            }
            _ => {}
        }
    }
    let type_args = inferred
        .into_iter()
        .enumerate()
        .map(|(index, ty)| {
            ty.ok_or_else(|| format!("cannot infer type parameter T{index} of {name}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    instantiate(module, callee, &type_args, actual)
}

/// Validate an already chosen substitution against the verified definition.
/// This is the cross-IR check for a concrete Core generic call instruction.
pub fn instantiate(
    module: &CheckedModule,
    callee: usize,
    type_args: &[Type],
    actual: &[Type],
) -> Result<ResolvedCall, String> {
    let function = module
        .as_module()
        .functions
        .get(callee)
        .ok_or_else(|| format!("invalid generic callee {callee}"))?;
    if type_args.len() != function.type_params {
        return Err(format!("{} type argument count mismatch", function.name));
    }
    if actual.len() != function.params.len() {
        return Err(format!("{} argument count mismatch", function.name));
    }
    for &ty in type_args {
        owning_type(ty)?;
    }
    let params = function
        .params
        .iter()
        .map(|ty| substitute(*ty, type_args))
        .collect::<Result<Vec<_>, _>>()?;
    if params != actual {
        return Err(format!("{} argument type mismatch", function.name));
    }
    Ok(ResolvedCall {
        callee,
        type_args: type_args.to_vec(),
        params,
        result: substitute(function.result, type_args)?,
    })
}

#[cfg(test)]
mod tests {
    use phoebe_generic_core as generic_core;

    use super::*;
    use crate::{
        Block, DataDef, DataId, Function, Instruction, Operation, ParamMode, Program, Span,
        Terminator, ValueId,
    };

    fn module(source: &str) -> CheckedModule {
        generic_core::lower_program_checked(&phoebe_syntax::parse(source).unwrap()).unwrap()
    }

    #[test]
    fn concrete_calls_infer_scalar_nominal_and_owning_right_substitutions() {
        let module = module("fn identity<T>(x: T) -> T = x");
        for ty in [Type::I64, Type::Data(DataId(0)), Type::SenderI64] {
            let call = infer(&module, "identity", &[ty]).unwrap();
            assert_eq!(call.type_args, vec![ty]);
            assert_eq!(call.params, vec![ty]);
            assert_eq!(call.result, ty);
            assert_eq!(
                instantiate(&module, call.callee, &call.type_args, &[ty]).unwrap(),
                call
            );
        }
    }

    #[test]
    fn signature_mismatch_and_uninferred_binder_are_diagnosed() {
        let checked = module("fn choose<T>(flag: Bool, x: T) -> T = if (flag) { x } else { x }");
        assert!(
            infer(&checked, "choose", &[Type::Bool])
                .unwrap_err()
                .contains("count")
        );
        assert!(
            infer(&checked, "choose", &[Type::I64, Type::I64])
                .unwrap_err()
                .contains("argument type mismatch")
        );
        let phantom = module("fn phantom<T>(x: I64) -> I64 = x");
        assert!(
            infer(&phantom, "phantom", &[Type::I64])
                .unwrap_err()
                .contains("cannot infer")
        );
    }

    #[test]
    fn owned_binder_rejects_borrowed_reference() {
        let module = module("fn identity<T>(x: T) -> T = x");
        for ty in [
            Type::RefI64,
            Type::RefMutI64,
            Type::RefBool,
            Type::RefMutBool,
        ] {
            assert!(
                infer(&module, "identity", &[ty])
                    .unwrap_err()
                    .contains("borrowed")
            );
            assert!(
                instantiate(&module, 0, &[ty], &[ty])
                    .unwrap_err()
                    .contains("borrowed")
            );
        }
    }

    fn linked_call_program(ty: Type, data: Vec<DataDef>, return_input: bool) -> Program {
        Program {
            data,
            functions: vec![Function {
                name: "entry".into(),
                param_modes: vec![ParamMode::Own],
                return_type: ty,
                blocks: vec![Block {
                    params: vec![ValueId(0)],
                    instructions: vec![Instruction {
                        result: Some(ValueId(1)),
                        operation: Operation::GenericCall {
                            callee: 0,
                            type_args: vec![ty],
                            args: vec![ValueId(0)],
                        },
                        span: Span { start: 1, end: 2 },
                    }],
                    terminator: Terminator::Return(if return_input {
                        ValueId(0)
                    } else {
                        ValueId(1)
                    }),
                }],
                value_types: vec![ty, ty],
                place_types: vec![],
                loan_regions: vec![],
            }],
        }
    }

    #[test]
    fn linked_core_call_checks_signature_without_weakening_plain_core() {
        let generics = module("fn identity<T>(x: T) -> T = x");
        let program = linked_call_program(Type::I64, vec![], false);
        assert!(
            crate::verify(&program)
                .unwrap_err()
                .contains("unlinked generic call")
        );
        crate::CheckedLinkedProgram::check(program.clone(), generics.clone()).unwrap();
        let mut wrong = program;
        let Operation::GenericCall { type_args, .. } =
            &mut wrong.functions[0].blocks[0].instructions[0].operation
        else {
            unreachable!()
        };
        *type_args = vec![Type::Bool];
        assert!(
            crate::CheckedLinkedProgram::check(wrong, generics)
                .err()
                .unwrap()
                .contains("argument type mismatch")
        );
    }

    #[test]
    fn linked_call_transfers_every_capability_leaf_and_rejects_reuse() {
        let generics = module("fn identity<T>(x: T) -> T = x");
        let data = vec![DataDef {
            name: "Envelope".into(),
            variants: vec![],
            payloads: vec![],
            fields: vec![("sender".into(), Type::SenderI64)],
        }];
        let good = linked_call_program(Type::Data(DataId(0)), data.clone(), false);
        crate::CheckedLinkedProgram::check(good, generics.clone()).unwrap();
        let reused = linked_call_program(Type::Data(DataId(0)), data, true);
        assert!(
            crate::CheckedLinkedProgram::check(reused, generics)
                .err()
                .unwrap()
                .contains("use after move")
        );
    }

    #[test]
    fn mixed_source_lowers_concrete_call_into_checked_linked_core() {
        let source =
            phoebe_syntax::parse("fn identity<T>(x: T) -> T = x\nfn main() -> I64 = identity(42)")
                .unwrap();
        let linked = crate::lower_linked_checked(&source).unwrap();
        assert_eq!(linked.program().functions.len(), 1);
        assert_eq!(linked.generics().as_module().functions.len(), 1);
        assert!(
            linked.program().functions[0].blocks[0]
                .instructions
                .iter()
                .any(|instruction| matches!(
                    &instruction.operation,
                    Operation::GenericCall {
                        callee: 0,
                        type_args,
                        ..
                    } if type_args == &[Type::I64]
                ))
        );
        assert_eq!(linked.interpret("main", &[]).unwrap(), 42);
        assert!(crate::lower(&source).is_err());
    }

    #[test]
    fn mixed_source_interprets_transitive_generic_branch() {
        let source = phoebe_syntax::parse(
            "fn choose<T>(flag: Bool, x: T) -> T = if (flag) { x } else { x }\nfn through<U>(flag: Bool, x: U) -> U = choose(flag, x)\nfn main() -> I64 = through(true, 41)",
        )
        .unwrap();
        let linked = crate::lower_linked_checked(&source).unwrap();
        assert_eq!(linked.interpret("main", &[]).unwrap(), 41);
    }

    #[test]
    fn mixed_source_rejects_recursive_generic_call_before_execution() {
        let source = phoebe_syntax::parse(
            "fn recurse<T>(x: T) -> T = recurse(x)\nfn main() -> I64 = recurse(1)",
        )
        .unwrap();
        assert!(
            crate::lower_linked_checked(&source)
                .err()
                .unwrap()
                .message
                .contains("recursive generic call cycle")
        );
    }

    #[test]
    fn mixed_source_reports_concrete_generic_signature_error_at_call() {
        let source =
            phoebe_syntax::parse("fn identity<T>(x: T) -> T = x\nfn main() -> I64 = identity()")
                .unwrap();
        let error = crate::lower_linked_checked(&source).err().unwrap();
        assert!(error.message.contains("count mismatch"));
        assert!(error.span.start > source.functions[1].name_span.start);
    }
}
