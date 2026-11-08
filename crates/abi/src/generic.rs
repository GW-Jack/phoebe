//! Transient, shape-shared emission plan for the verified parametric Core
//! subset. This is not a persistent object-cache key: the module identity and
//! target are fixed for one planning session.

use std::collections::HashMap;

use phoebe_core::Type as ConcreteType;
use phoebe_generic_core::{self as generic_core, CheckedModule, Operation, Type as GenericType};

use crate::{Shape, Shapes};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TargetAbi {
    pub triple: String,
    pub direct_register_results: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SignatureShape {
    pub params: Vec<Shape>,
    pub result: Shape,
    pub return_buffer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InstanceKey {
    pub function: usize,
    pub target: TargetAbi,
    pub signature: SignatureShape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceCall {
    pub block: usize,
    pub instruction: usize,
    pub target: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub key: InstanceKey,
    /// Representative binder layouts for code generation. They are omitted
    /// from the sharing key because unused binders cannot affect this IR.
    pub binders: Vec<Shape>,
    pub calls: Vec<InstanceCall>,
}

pub struct Planner<'a> {
    module: &'a generic_core::Module,
    shapes: &'a Shapes,
    target: TargetAbi,
    instances: Vec<Instance>,
    by_key: HashMap<InstanceKey, usize>,
}

fn concrete(ty: GenericType, args: &[ConcreteType]) -> Result<ConcreteType, String> {
    match ty {
        GenericType::Param(index) => args
            .get(index)
            .copied()
            .ok_or_else(|| format!("missing generic type argument T{index}")),
        GenericType::I64 => Ok(ConcreteType::I64),
        GenericType::Bool => Ok(ConcreteType::Bool),
        GenericType::Unit => Ok(ConcreteType::Unit),
    }
}

fn check_acyclic(module: &generic_core::Module) -> Result<(), String> {
    let edges = module
        .functions
        .iter()
        .map(|function| {
            function
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter_map(|instruction| match &instruction.operation {
                    Operation::Call { callee, .. } => Some(*callee),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut state = vec![0; module.functions.len()];
    for index in 0..module.functions.len() {
        if state[index] != 0 {
            continue;
        }
        state[index] = 1;
        let mut stack = vec![(index, 0)];
        while let Some((current, offset)) = stack.last_mut() {
            if *offset == edges[*current].len() {
                state[*current] = 2;
                stack.pop();
                continue;
            }
            let callee = edges[*current][*offset];
            *offset += 1;
            match state[callee] {
                1 => {
                    return Err(format!(
                        "recursive generic call cycle includes {}",
                        module.functions[callee].name
                    ));
                }
                0 => {
                    state[callee] = 1;
                    stack.push((callee, 0));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

impl<'a> Planner<'a> {
    /// Check all generic bodies once, then plan concrete uses by ABI shape.
    /// The current IR has owned parameters, no dictionaries, and no
    /// suspension, so equal signatures of one body can share native code.
    pub fn new(
        module: &'a generic_core::Module,
        shapes: &'a Shapes,
        target: TargetAbi,
    ) -> Result<Self, String> {
        generic_core::verify_module(module)?;
        Self::from_verified(module, shapes, target)
    }

    /// Use a module whose body proof was already retained by source lowering.
    pub fn from_checked(
        module: &'a CheckedModule,
        shapes: &'a Shapes,
        target: TargetAbi,
    ) -> Result<Self, String> {
        Self::from_verified(module.as_module(), shapes, target)
    }

    fn from_verified(
        module: &'a generic_core::Module,
        shapes: &'a Shapes,
        target: TargetAbi,
    ) -> Result<Self, String> {
        check_acyclic(module)?;
        Ok(Self {
            module,
            shapes,
            target,
            instances: Vec::new(),
            by_key: HashMap::new(),
        })
    }

    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    pub fn module(&self) -> &generic_core::Module {
        self.module
    }

    pub fn target(&self) -> &TargetAbi {
        &self.target
    }

    fn key(&self, function: usize, type_args: &[ConcreteType]) -> Result<InstanceKey, String> {
        let definition = self
            .module
            .functions
            .get(function)
            .ok_or_else(|| format!("invalid generic function {function}"))?;
        if type_args.len() != definition.type_params {
            return Err(format!("{} type argument count mismatch", definition.name));
        }
        if type_args
            .iter()
            .any(|ty| matches!(ty, ConcreteType::RefI64 | ConcreteType::RefMutI64))
        {
            return Err("borrowed reference cannot instantiate an owned type parameter".into());
        }
        let params = definition
            .params
            .iter()
            .map(|ty| self.shapes.shape(concrete(*ty, type_args)?))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self.shapes.shape(concrete(definition.result, type_args)?)?;
        Ok(InstanceKey {
            function,
            target: self.target.clone(),
            signature: SignatureShape {
                params,
                return_buffer: result.parts().len() > self.target.direct_register_results,
                result,
            },
        })
    }

    pub fn plan(&mut self, function: usize, type_args: &[ConcreteType]) -> Result<usize, String> {
        enum Work {
            Discover(usize, Vec<ConcreteType>),
            Finish(InstanceKey, Vec<Shape>, Vec<(usize, usize, InstanceKey)>),
        }
        let root = self.key(function, type_args)?;
        let mut work = vec![Work::Discover(function, type_args.to_vec())];
        while let Some(item) = work.pop() {
            match item {
                Work::Discover(function, args) => {
                    let key = self.key(function, &args)?;
                    if self.by_key.contains_key(&key) {
                        continue;
                    }
                    let mut calls = Vec::new();
                    let mut children = Vec::new();
                    for (block_id, block) in
                        self.module.functions[function].blocks.iter().enumerate()
                    {
                        for (instruction_id, instruction) in block.instructions.iter().enumerate() {
                            if let Operation::Call {
                                callee,
                                type_args: callee_args,
                                ..
                            } = &instruction.operation
                            {
                                let concrete_args = callee_args
                                    .iter()
                                    .map(|ty| concrete(*ty, &args))
                                    .collect::<Result<Vec<_>, _>>()?;
                                let child_key = self.key(*callee, &concrete_args)?;
                                calls.push((block_id, instruction_id, child_key));
                                children.push((*callee, concrete_args));
                            }
                        }
                    }
                    let binders = args
                        .iter()
                        .map(|ty| self.shapes.shape(*ty))
                        .collect::<Result<Vec<_>, _>>()?;
                    work.push(Work::Finish(key, binders, calls));
                    for (callee, concrete_args) in children.into_iter().rev() {
                        work.push(Work::Discover(callee, concrete_args));
                    }
                }
                Work::Finish(key, binders, pending_calls) => {
                    if self.by_key.contains_key(&key) {
                        continue;
                    }
                    let calls = pending_calls
                        .into_iter()
                        .map(|(block, instruction, child)| {
                            self.by_key
                                .get(&child)
                                .copied()
                                .map(|target| InstanceCall {
                                    block,
                                    instruction,
                                    target,
                                })
                                .ok_or("missing planned generic callee".to_string())
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let index = self.instances.len();
                    self.instances.push(Instance {
                        key: key.clone(),
                        binders,
                        calls,
                    });
                    self.by_key.insert(key, index);
                }
            }
        }
        self.by_key
            .get(&root)
            .copied()
            .ok_or("missing planned generic root".into())
    }
}

#[cfg(test)]
mod tests {
    use phoebe_core::{DataDef, DataId, Program, Type};

    use super::*;

    fn generic_module(source: &str) -> generic_core::Module {
        let ast = phoebe_syntax::parse(source).unwrap();
        generic_core::lower_program(&ast).unwrap()
    }

    fn shapes() -> Shapes {
        Shapes::new(&Program {
            data: vec![
                DataDef {
                    name: "First".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("a".into(), Type::I64),
                        ("b".into(), Type::Bool),
                        ("c".into(), Type::I64),
                    ],
                },
                DataDef {
                    name: "Second".into(),
                    variants: vec![],
                    payloads: vec![],
                    fields: vec![
                        ("x".into(), Type::I64),
                        ("y".into(), Type::Bool),
                        ("z".into(), Type::I64),
                    ],
                },
            ],
            functions: vec![],
        })
        .unwrap()
    }

    #[test]
    fn equal_shapes_share_one_body_and_transitive_callee() {
        let source = phoebe_syntax::parse(
            "fn identity<T>(x: T) -> T = x\nfn through<U>(x: U) -> U = identity(x)",
        )
        .unwrap();
        let module = generic_core::lower_program_checked(&source).unwrap();
        let layouts = shapes();
        let mut planner = Planner::from_checked(
            &module,
            &layouts,
            TargetAbi {
                triple: "test-aarch64".into(),
                direct_register_results: 8,
            },
        )
        .unwrap();
        let first = planner.plan(1, &[Type::Data(DataId(0))]).unwrap();
        let second = planner.plan(1, &[Type::Data(DataId(1))]).unwrap();
        assert_eq!(first, second);
        assert_eq!(planner.instances().len(), 2);
        assert_eq!(planner.instances()[first].calls[0].target, 0);
        let bool_instance = planner.plan(1, &[Type::Bool]).unwrap();
        assert_ne!(first, bool_instance);
        assert_eq!(planner.instances().len(), 4);
    }

    #[test]
    fn opaque_word_identity_shares_across_capability_and_scalar() {
        let module = generic_module("fn identity<T>(x: T) -> T = x");
        let layouts = shapes();
        let mut planner = Planner::new(
            &module,
            &layouts,
            TargetAbi {
                triple: "test-x86_64".into(),
                direct_register_results: 2,
            },
        )
        .unwrap();
        let scalar = planner.plan(0, &[Type::I64]).unwrap();
        let right = planner.plan(0, &[Type::SenderI64]).unwrap();
        assert_eq!(scalar, right);
        assert_eq!(planner.instances().len(), 1);
        let product = planner.plan(0, &[Type::Data(DataId(0))]).unwrap();
        assert_ne!(scalar, product);
        assert!(planner.instances()[product].key.signature.return_buffer);
    }

    #[test]
    fn recursive_generic_call_graph_is_rejected_before_planning() {
        let module = generic_module("fn recur<T>(x: T) -> T = recur(x)");
        let layouts = shapes();
        assert!(
            Planner::new(
                &module,
                &layouts,
                TargetAbi {
                    triple: "test".into(),
                    direct_register_results: 2,
                },
            )
            .err()
            .unwrap()
            .contains("recursive generic call cycle")
        );
    }

    #[test]
    fn borrowed_reference_cannot_instantiate_owned_binder() {
        let module = generic_module("fn identity<T>(x: T) -> T = x");
        let layouts = shapes();
        let mut planner = Planner::new(
            &module,
            &layouts,
            TargetAbi {
                triple: "test".into(),
                direct_register_results: 2,
            },
        )
        .unwrap();
        assert!(
            planner
                .plan(0, &[Type::RefI64])
                .unwrap_err()
                .contains("borrowed reference")
        );
        assert!(planner.instances().is_empty());
    }

    #[test]
    fn long_generic_call_chain_plans_without_recursive_traversal() {
        let mut source = "fn f0<T>(x: T) -> T = x\n".to_string();
        for index in 1..256 {
            source.push_str(&format!("fn f{index}<T>(x: T) -> T = f{}(x)\n", index - 1));
        }
        let module = generic_module(&source);
        let layouts = shapes();
        let mut planner = Planner::new(
            &module,
            &layouts,
            TargetAbi {
                triple: "test".into(),
                direct_register_results: 2,
            },
        )
        .unwrap();
        let root = planner.plan(255, &[Type::I64]).unwrap();
        assert_eq!(root, 255);
        assert_eq!(planner.instances().len(), 256);
    }
}
