//! Exact source-level interface used to decide whether importers must rebuild.

use std::collections::HashSet;

use phoebe_syntax::syntax::{self, ParamMode, TypeName};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleInterface {
    pub name: String,
    pub data: Vec<DataInterface>,
    pub functions: Vec<FunctionInterface>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataInterface {
    pub name: String,
    pub variants: Vec<(String, Option<(String, TypeName)>)>,
    pub fields: Vec<(String, TypeName)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionInterface {
    pub name: String,
    pub params: Vec<(String, ParamMode, TypeName)>,
    pub return_type: TypeName,
}

pub(super) fn from_program(name: &str, program: &syntax::Program) -> ModuleInterface {
    fn collect_type(ty: &TypeName, needed: &mut HashSet<String>) {
        match ty {
            TypeName::Named(name) => {
                needed.insert(name.clone());
            }
            TypeName::Result(ok, err) => {
                collect_type(ok, needed);
                collect_type(err, needed);
            }
            _ => {}
        }
    }

    // A private nominal type may still determine the representation of an
    // exported signature. Include its shape transitively without exporting
    // its name to source importers.
    let mut needed = program
        .data
        .iter()
        .filter(|data| data.public)
        .map(|data| data.name.clone())
        .collect::<HashSet<_>>();
    for function in program.functions.iter().filter(|function| function.public) {
        for param in &function.params {
            collect_type(&param.ty, &mut needed);
        }
        collect_type(&function.return_type, &mut needed);
    }
    loop {
        let before = needed.len();
        for data in &program.data {
            if !needed.contains(&data.name) {
                continue;
            }
            for variant in &data.variants {
                if let Some(payload) = &variant.payload {
                    collect_type(&payload.ty, &mut needed);
                }
            }
            for field in &data.fields {
                collect_type(&field.ty, &mut needed);
            }
        }
        if needed.len() == before {
            break;
        }
    }
    ModuleInterface {
        name: name.to_owned(),
        data: program
            .data
            .iter()
            .filter(|data| needed.contains(&data.name))
            .map(|data| DataInterface {
                name: data.name.clone(),
                variants: data
                    .variants
                    .iter()
                    .map(|variant| {
                        (
                            variant.name.clone(),
                            variant
                                .payload
                                .as_ref()
                                .map(|field| (field.name.clone(), field.ty.clone())),
                        )
                    })
                    .collect(),
                fields: data
                    .fields
                    .iter()
                    .map(|field| (field.name.clone(), field.ty.clone()))
                    .collect(),
            })
            .collect(),
        functions: program
            .functions
            .iter()
            .filter(|function| function.public)
            .map(|function| FunctionInterface {
                name: function.name.clone(),
                params: function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.mode, param.ty.clone()))
                    .collect(),
                return_type: function.return_type.clone(),
            })
            .collect(),
    }
}
