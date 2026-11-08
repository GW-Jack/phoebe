//! Concrete type substitution for a verified parametric call signature.
//! This performs work proportional to the call signature, never the callee
//! body. It does not emit a Core call instruction or execute the callee.

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
    if matches!(ty, Type::RefI64 | Type::RefMutI64) {
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
/// This is the cross-IR check a future concrete Core call instruction needs.
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
    use crate::DataId;

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
        for ty in [Type::RefI64, Type::RefMutI64] {
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
}
