//! Exact source-level interface used to decide whether importers must rebuild.

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
    ModuleInterface {
        name: name.to_owned(),
        data: program
            .data
            .iter()
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
