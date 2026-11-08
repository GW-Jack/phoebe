//! Exact source-level interface used to decide whether importers must rebuild.

use std::collections::HashSet;

use phoebe_syntax::syntax::{self, ParamMode, TypeName};
use sha2::{Digest, Sha256};

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
    pub params: Vec<(ParamMode, TypeName)>,
    pub return_type: TypeName,
}

impl ModuleInterface {
    /// Versioned, unambiguous source interface encoding for future disk keys.
    /// Declaration order is retained because the current Core ABI assigns
    /// nominal IDs and function indices in that order.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn length(bytes: &mut Vec<u8>, value: usize) {
            bytes.extend_from_slice(&(value as u64).to_le_bytes());
        }
        fn name(bytes: &mut Vec<u8>, value: &str) {
            length(bytes, value.len());
            bytes.extend_from_slice(value.as_bytes());
        }
        fn ty(bytes: &mut Vec<u8>, value: &TypeName) {
            match value {
                TypeName::I64 => bytes.push(0),
                TypeName::Bool => bytes.push(1),
                TypeName::SenderI64 => bytes.push(2),
                TypeName::ReceiverI64 => bytes.push(3),
                TypeName::ReplyToI64 => bytes.push(4),
                TypeName::ReplyReceiverI64 => bytes.push(5),
                TypeName::RequestI64 => bytes.push(6),
                TypeName::RequestSenderI64 => bytes.push(7),
                TypeName::RequestReceiverI64 => bytes.push(8),
                TypeName::Named(value) => {
                    bytes.push(9);
                    name(bytes, value);
                }
                TypeName::Result(ok, err) => {
                    bytes.push(10);
                    ty(bytes, ok);
                    ty(bytes, err);
                }
            }
        }
        let mut bytes = b"PHOEBE-INTERFACE\0\x01".to_vec();
        name(&mut bytes, &self.name);
        length(&mut bytes, self.data.len());
        for data in &self.data {
            name(&mut bytes, &data.name);
            length(&mut bytes, data.variants.len());
            for (variant, payload) in &data.variants {
                name(&mut bytes, variant);
                match payload {
                    Some((field, field_ty)) => {
                        bytes.push(1);
                        name(&mut bytes, field);
                        ty(&mut bytes, field_ty);
                    }
                    None => bytes.push(0),
                }
            }
            length(&mut bytes, data.fields.len());
            for (field, field_ty) in &data.fields {
                name(&mut bytes, field);
                ty(&mut bytes, field_ty);
            }
        }
        length(&mut bytes, self.functions.len());
        for function in &self.functions {
            name(&mut bytes, &function.name);
            length(&mut bytes, function.params.len());
            for (mode, param_ty) in &function.params {
                bytes.push(match mode {
                    ParamMode::Own => 0,
                    ParamMode::Shared => 1,
                    ParamMode::Exclusive => 2,
                });
                ty(&mut bytes, param_ty);
            }
            ty(&mut bytes, &function.return_type);
        }
        bytes
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }
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
                    .map(|param| (param.mode, param.ty.clone()))
                    .collect(),
                return_type: function.return_type.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(source: &str) -> ModuleInterface {
        from_program("sample", &syntax::parse(source).unwrap())
    }

    #[test]
    fn fingerprint_tracks_exported_facts_without_body_text() {
        let before = interface(
            "pub data Boxed { value: I64 }\n\
             pub fn answer(boxed: Boxed) -> I64 = boxed.value\n",
        );
        let changed_body = interface(
            "pub data Boxed { value: I64 }\n\
             pub fn answer(boxed: Boxed) -> I64 = 42\n",
        );
        assert_eq!(before.canonical_bytes(), changed_body.canonical_bytes());
        assert_eq!(before.fingerprint(), changed_body.fingerprint());
        let changed_shape = interface(
            "pub data Boxed { value: Bool }\n\
             pub fn answer(boxed: Boxed) -> I64 = 42\n",
        );
        assert_ne!(before.canonical_bytes(), changed_shape.canonical_bytes());
        assert_ne!(before.fingerprint(), changed_shape.fingerprint());
    }

    #[test]
    fn fingerprint_includes_reachable_private_layout_and_module_identity() {
        let before = interface(
            "data Hidden { value: I64 }\n\
             pub fn read(value: Hidden) -> I64 = value.value\n",
        );
        let changed = interface(
            "data Hidden { value: Bool }\n\
             pub fn read(value: Hidden) -> I64 = 42\n",
        );
        assert_ne!(before.fingerprint(), changed.fingerprint());
        let mut renamed = before.clone();
        renamed.name = "other".into();
        assert_ne!(before.fingerprint(), renamed.fingerprint());
    }

    #[test]
    fn length_prefixes_distinguish_ambiguous_name_concatenations() {
        let a = ModuleInterface {
            name: "ab".into(),
            data: vec![],
            functions: vec![FunctionInterface {
                name: "c".into(),
                params: vec![],
                return_type: TypeName::I64,
            }],
        };
        let b = ModuleInterface {
            name: "a".into(),
            data: vec![],
            functions: vec![FunctionInterface {
                name: "bc".into(),
                params: vec![],
                return_type: TypeName::I64,
            }],
        };
        assert_ne!(a.canonical_bytes(), b.canonical_bytes());
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
