//! Exact source-level interface used to decide whether importers must rebuild.

use std::collections::{HashMap, HashSet};

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
    pub type_params: Vec<String>,
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
        fn ty(bytes: &mut Vec<u8>, value: &TypeName, binders: &HashMap<&str, usize>) {
            match value {
                TypeName::I64 => bytes.push(0),
                TypeName::ArrayI64(len) => {
                    bytes.push(18);
                    length(bytes, *len);
                }
                TypeName::Bool => bytes.push(1),
                TypeName::Unit => bytes.push(12),
                TypeName::SenderI64 => bytes.push(2),
                TypeName::ReceiverI64 => bytes.push(3),
                TypeName::ReplyToI64 => bytes.push(4),
                TypeName::ReplyReceiverI64 => bytes.push(5),
                TypeName::RequestI64 => bytes.push(6),
                TypeName::RequestSenderI64 => bytes.push(7),
                TypeName::RequestReceiverI64 => bytes.push(8),
                TypeName::Named(value) => match binders.get(value.as_str()) {
                    Some(index) => {
                        bytes.push(13);
                        length(bytes, *index);
                    }
                    None => {
                        bytes.push(9);
                        name(bytes, value);
                    }
                },
                TypeName::Result(ok, err) => {
                    bytes.push(10);
                    ty(bytes, ok, binders);
                    ty(bytes, err, binders);
                }
                TypeName::Option(value) => {
                    bytes.push(11);
                    ty(bytes, value, binders);
                }
                TypeName::TypedSender(value) => {
                    bytes.push(14);
                    ty(bytes, value, binders);
                }
                TypeName::TypedReceiver(value) => {
                    bytes.push(15);
                    ty(bytes, value, binders);
                }
                TypeName::Send(value) => {
                    bytes.push(16);
                    ty(bytes, value, binders);
                }
                TypeName::Receive(value) => {
                    bytes.push(17);
                    ty(bytes, value, binders);
                }
            }
        }
        let mut bytes = b"PHOEBE-INTERFACE\0\x03".to_vec();
        let no_binders = HashMap::new();
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
                        ty(&mut bytes, field_ty, &no_binders);
                    }
                    None => bytes.push(0),
                }
            }
            length(&mut bytes, data.fields.len());
            for (field, field_ty) in &data.fields {
                name(&mut bytes, field);
                ty(&mut bytes, field_ty, &no_binders);
            }
        }
        length(&mut bytes, self.functions.len());
        for function in &self.functions {
            name(&mut bytes, &function.name);
            length(&mut bytes, function.type_params.len());
            let binders = function
                .type_params
                .iter()
                .enumerate()
                .map(|(index, name)| (name.as_str(), index))
                .collect::<HashMap<_, _>>();
            length(&mut bytes, function.params.len());
            for (mode, param_ty) in &function.params {
                bytes.push(match mode {
                    ParamMode::Own => 0,
                    ParamMode::Shared => 1,
                    ParamMode::Exclusive => 2,
                });
                ty(&mut bytes, param_ty, &binders);
            }
            ty(&mut bytes, &function.return_type, &binders);
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
            TypeName::Option(value) => collect_type(value, needed),
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
        let mut function_types = HashSet::new();
        for param in &function.params {
            collect_type(&param.ty, &mut function_types);
        }
        collect_type(&function.return_type, &mut function_types);
        for (param, _) in &function.type_params {
            function_types.remove(param);
        }
        needed.extend(function_types);
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
                type_params: function
                    .type_params
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect(),
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
    fn fingerprint_tracks_fixed_array_lengths_in_signatures_and_fields() {
        let before = interface(
            "pub data Pair { words: [I64; 2] }\npub fn id(xs: [I64; 2]) -> [I64; 2] = xs",
        );
        let changed_param = interface(
            "pub data Pair { words: [I64; 2] }\npub fn id(xs: [I64; 3]) -> [I64; 2] = [1, 2]",
        );
        let changed_field = interface(
            "pub data Pair { words: [I64; 3] }\npub fn id(xs: [I64; 2]) -> [I64; 2] = xs",
        );
        assert_ne!(before.fingerprint(), changed_param.fingerprint());
        assert_ne!(before.fingerprint(), changed_field.fingerprint());
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
                type_params: vec![],
                params: vec![],
                return_type: TypeName::I64,
            }],
        };
        let b = ModuleInterface {
            name: "a".into(),
            data: vec![],
            functions: vec![FunctionInterface {
                name: "bc".into(),
                type_params: vec![],
                params: vec![],
                return_type: TypeName::I64,
            }],
        };
        assert_ne!(a.canonical_bytes(), b.canonical_bytes());
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn public_generic_binders_are_alpha_normalized_but_arity_is_tracked() {
        let before = interface("pub fn identity<T>(value: T) -> T = value");
        let renamed = interface("pub fn identity<U>(value: U) -> U = value");
        assert_eq!(before.fingerprint(), renamed.fingerprint());
        assert_eq!(before.data, renamed.data);
        let expanded = interface("pub fn identity<T, U>(value: U) -> U = value");
        assert_ne!(before.fingerprint(), expanded.fingerprint());
    }
}
