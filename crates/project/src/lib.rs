//! Manifest loading and module name resolution for a local Phoebe package.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use phoebe_syntax::syntax::{self, Body, CallArgument, Expr, ExprKind, Statement, TypeName};
use phoebe_syntax::{Diagnostic, Span};

mod graph;
mod interface;
mod session;

pub use interface::{DataInterface, FunctionInterface, ModuleInterface};
pub use session::{LoadReport, ProjectSession};

struct SourceFile {
    path: PathBuf,
    start: usize,
    source: String,
}

pub struct Loaded {
    pub ast: syntax::Program,
    pub interfaces: Vec<ModuleInterface>,
    pub function_modules: Vec<String>,
    sources: Vec<SourceFile>,
}

impl Loaded {
    pub fn render(&self, diagnostic: &Diagnostic) -> String {
        let file = self
            .sources
            .iter()
            .rev()
            .find(|file| diagnostic.span.start >= file.start)
            .expect("project has at least one source file");
        let local = Diagnostic::new(
            Span {
                start: diagnostic.span.start - file.start,
                end: diagnostic.span.end.saturating_sub(file.start),
            },
            &diagnostic.message,
        );
        format!("{}:{}", file.path.display(), local.render(&file.source))
    }
}

#[derive(Clone)]
struct Module {
    name: String,
    package_root: String,
    available: HashMap<String, String>,
    path: PathBuf,
    source: String,
    ast: syntax::Program,
}

#[derive(Clone)]
struct ExportSet {
    data: HashSet<String>,
    functions: HashSet<String>,
}

struct ResolvedImport {
    target: String,
    exports: ExportSet,
}

fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn manifest_string<'a>(table: &'a toml::Table, key: &str) -> Result<&'a str, String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("manifest requires string `{key}`"))
}

pub fn load(path: &Path) -> Result<Loaded, String> {
    load_with_cache(path, None).map(|parts| parts.loaded)
}

struct LoadParts {
    loaded: Loaded,
    modules: BTreeMap<String, Module>,
    parsed: Vec<String>,
}

fn load_with_cache(
    path: &Path,
    cache: Option<&BTreeMap<String, Module>>,
) -> Result<LoadParts, String> {
    let manifest_path = if path.is_dir() {
        path.join("phoebe.toml")
    } else {
        path.to_owned()
    };
    let source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let manifest: toml::Table =
        toml::from_str(&source).map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or("manifest requires [package]")?;
    let name = manifest_string(package, "name")?;
    if !identifier(name) {
        return Err("package name must be an identifier".into());
    }
    let root = manifest_string(package, "root")?.to_owned();
    let module_table = manifest
        .get("modules")
        .and_then(toml::Value::as_table)
        .ok_or("manifest requires [modules]")?;
    if !module_table.contains_key(&root) {
        return Err(format!("root module `{root}` is not listed in [modules]"));
    }
    let directory = manifest_path.parent().unwrap_or(Path::new("."));
    let mut parsed = Vec::new();
    let specs = if manifest.contains_key("dependencies") {
        graph::sources(&manifest_path, source.as_bytes())?.1
    } else {
        let available = module_table
            .keys()
            .map(|name| (name.clone(), name.clone()))
            .collect::<HashMap<_, _>>();
        module_table
            .iter()
            .map(|(name, path)| {
                if !identifier(name) {
                    return Err(format!("invalid module name `{name}`"));
                }
                let relative = path
                    .as_str()
                    .ok_or_else(|| format!("module `{name}` path must be a string"))?;
                Ok(graph::SourceSpec {
                    name: name.clone(),
                    path: directory.join(relative),
                    source: None,
                    package_root: root.clone(),
                    available: available.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?
    };
    let mut modules = specs
        .into_iter()
        .map(|spec| {
            let name = spec.name;
            if !identifier(&name) {
                return Err(format!("invalid module name `{name}`"));
            }
            let path = spec.path;
            let source = if let Some(source) = spec.source {
                source
            } else {
                fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?
            };
            let ast = if let Some(previous) = cache
                .and_then(|cache| cache.get(&name))
                .filter(|previous| previous.path == path && previous.source == source)
            {
                previous.ast.clone()
            } else {
                parsed.push(name.clone());
                syntax::parse(&source)
                    .map_err(|error| format!("{}:{}", path.display(), error.render(&source)))?
            };
            Ok(Module {
                name,
                package_root: spec.package_root,
                available: spec.available,
                path,
                source,
                ast,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    let names = modules
        .iter()
        .map(|module| module.name.clone())
        .collect::<HashSet<_>>();
    if names.len() != modules.len() {
        return Err("duplicate module name".into());
    }
    for module in &modules {
        let mut seen = HashSet::new();
        for import in &module.ast.imports {
            let Some(target) = module.available.get(&import.name) else {
                return Err(format!(
                    "{}:{}",
                    module.path.display(),
                    Diagnostic::new(import.span, format!("unknown module {}", import.name))
                        .render(&module.source)
                ));
            };
            if !names.contains(target) {
                return Err(format!("unresolved module `{}`", import.name));
            }
            if target == &module.name
                || target == &module.package_root
                || !seen.insert(&import.name)
            {
                return Err(format!(
                    "{}:{}",
                    module.path.display(),
                    Diagnostic::new(import.span, "self or duplicate import").render(&module.source)
                ));
            }
        }
    }
    let order = check_import_cycles(&modules)?;
    let positions = order
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name, index))
        .collect::<HashMap<_, _>>();
    modules.sort_by_key(|module| positions[&module.name]);
    let raw_modules = modules
        .iter()
        .cloned()
        .map(|module| (module.name.clone(), module))
        .collect::<BTreeMap<_, _>>();
    let exports = modules
        .iter()
        .map(|module| {
            (
                module.name.clone(),
                ExportSet {
                    data: module
                        .ast
                        .data
                        .iter()
                        .filter(|data| data.public)
                        .map(|data| data.name.clone())
                        .collect(),
                    functions: module
                        .ast
                        .functions
                        .iter()
                        .filter(|function| function.public)
                        .map(|function| function.name.clone())
                        .collect(),
                },
            )
        })
        .collect::<HashMap<_, _>>();

    let mut combined = syntax::Program {
        imports: Vec::new(),
        data: Vec::new(),
        functions: Vec::new(),
        applied_types: Vec::new(),
        uses_send_outcome: false,
        uses_receive_outcome: false,
        uses_complete_outcome: false,
        uses_reply_outcome: false,
    };
    let mut sources = Vec::new();
    let mut interfaces = Vec::new();
    let mut function_modules = Vec::new();
    let mut offset = 0;
    for mut module in modules {
        let path = &module.path;
        let source = &module.source;
        let local_data = module
            .ast
            .data
            .iter()
            .map(|item| item.name.clone())
            .collect::<HashSet<_>>();
        let local_functions = module
            .ast
            .functions
            .iter()
            .map(|item| item.name.clone())
            .collect::<HashSet<_>>();
        let imports = module
            .ast
            .imports
            .iter()
            .map(|item| {
                let target = module.available[&item.name].clone();
                (
                    item.name.clone(),
                    ResolvedImport {
                        exports: exports[&target].clone(),
                        target,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        if let Some(collision) = imports.keys().find(|name| local_data.contains(*name)) {
            return Err(format!(
                "{}: imported module `{collision}` conflicts with a local data type",
                module.path.display()
            ));
        }
        let mut application_names = HashMap::new();
        let no_binders = HashSet::new();
        for (ty, span) in &mut module.ast.applied_types {
            let before = ty.key();
            qualify_type(
                ty,
                &module.name,
                &no_binders,
                &local_data,
                &imports,
                &module.available,
            )
            .map_err(|message| source_error(path, source, *span, message))?;
            application_names.insert(before, ty.key());
        }
        for definition in &mut module.ast.data {
            for variant in &mut definition.variants {
                if let Some(payload) = &mut variant.payload {
                    qualify_type(
                        &mut payload.ty,
                        &module.name,
                        &no_binders,
                        &local_data,
                        &imports,
                        &module.available,
                    )
                    .map_err(|message| source_error(path, source, payload.span, message))?;
                }
            }
            for field in &mut definition.fields {
                qualify_type(
                    &mut field.ty,
                    &module.name,
                    &no_binders,
                    &local_data,
                    &imports,
                    &module.available,
                )
                .map_err(|message| source_error(path, source, field.span, message))?;
            }
            definition.name = format!("{}.{}", module.name, definition.name);
        }
        for function in &mut module.ast.functions {
            let binders = function
                .type_params
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<HashSet<_>>();
            for param in &mut function.params {
                qualify_type(
                    &mut param.ty,
                    &module.name,
                    &binders,
                    &local_data,
                    &imports,
                    &module.available,
                )
                .map_err(|message| source_error(path, source, param.span, message))?;
            }
            qualify_type(
                &mut function.return_type,
                &module.name,
                &binders,
                &local_data,
                &imports,
                &module.available,
            )
            .map_err(|message| source_error(path, source, function.name_span, message))?;
            rewrite_body(
                &mut function.body,
                &module.name,
                &root,
                &local_data,
                &local_functions,
                &imports,
                &module.available,
                &application_names,
            )
            .map_err(|(span, message)| source_error(path, source, span, message))?;
            function.name = symbol(&module.name, &function.name, &root);
        }
        interfaces.push(interface::from_program(&module.name, &module.ast));
        function_modules.extend(std::iter::repeat_n(
            module.name.clone(),
            module.ast.functions.len(),
        ));
        shift_program(&mut module.ast, offset);
        combined.data.extend(module.ast.data);
        combined.functions.extend(module.ast.functions);
        combined.applied_types.extend(module.ast.applied_types);
        combined.uses_send_outcome |= module.ast.uses_send_outcome;
        combined.uses_receive_outcome |= module.ast.uses_receive_outcome;
        combined.uses_complete_outcome |= module.ast.uses_complete_outcome;
        combined.uses_reply_outcome |= module.ast.uses_reply_outcome;
        sources.push(SourceFile {
            path: module.path,
            start: offset,
            source: module.source,
        });
        offset += sources.last().expect("source inserted").source.len() + 1;
    }
    if !combined
        .functions
        .iter()
        .any(|function| function.name == "main")
    {
        return Err(format!("root module `{root}` requires fn main"));
    }
    Ok(LoadParts {
        loaded: Loaded {
            ast: combined,
            interfaces,
            function_modules,
            sources,
        },
        modules: raw_modules,
        parsed,
    })
}

fn source_error(path: &Path, source: &str, span: Span, message: String) -> String {
    format!(
        "{}:{}",
        path.display(),
        Diagnostic::new(span, message).render(source)
    )
}

fn check_import_cycles(modules: &[Module]) -> Result<Vec<String>, String> {
    let graph = modules
        .iter()
        .map(|module| {
            (
                module.name.as_str(),
                module
                    .ast
                    .imports
                    .iter()
                    .map(|import| module.available[&import.name].as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    // An explicit DFS stack keeps a long but valid import chain from using
    // the compiler's process stack. Preserve the old dependency-first order:
    // roots are sorted, and each module's imports retain source order.
    let mut visiting = HashSet::new();
    let mut done = HashSet::new();
    let mut order = Vec::new();
    for &root in graph.keys() {
        if done.contains(root) {
            continue;
        }
        visiting.insert(root);
        let mut stack = vec![(root, 0usize)];
        while let Some((name, next)) = stack.last_mut() {
            let imports = &graph[name];
            if *next == imports.len() {
                let name = *name;
                stack.pop();
                visiting.remove(name);
                done.insert(name);
                order.push(name.to_owned());
                continue;
            }
            let child = imports[*next];
            *next += 1;
            if done.contains(child) {
                continue;
            }
            if !visiting.insert(child) {
                return Err(format!("module import cycle includes `{child}`"));
            }
            stack.push((child, 0));
        }
    }
    Ok(order)
}

fn symbol(module: &str, name: &str, root: &str) -> String {
    if module == root && name == "main" {
        "main".into()
    } else {
        format!("{module}.{name}")
    }
}

fn qualify_type(
    ty: &mut TypeName,
    module: &str,
    binders: &HashSet<String>,
    local_data: &HashSet<String>,
    imports: &HashMap<String, ResolvedImport>,
    available: &HashMap<String, String>,
) -> Result<(), String> {
    match ty {
        TypeName::I64
        | TypeName::ArrayI64(_)
        | TypeName::Bool
        | TypeName::Unit
        | TypeName::SenderI64
        | TypeName::ReceiverI64
        | TypeName::ReplyToI64
        | TypeName::ReplyReceiverI64
        | TypeName::RequestI64
        | TypeName::RequestSenderI64
        | TypeName::RequestReceiverI64 => {}
        TypeName::Named(name) if binders.contains(name) => {}
        TypeName::Named(name) if local_data.contains(name) => {
            *name = format!("{module}.{name}");
        }
        TypeName::Named(name) => qualify_imported_data_name(name, imports, available)?,
        TypeName::Option(value)
        | TypeName::TypedSender(value)
        | TypeName::TypedReceiver(value)
        | TypeName::Send(value)
        | TypeName::Receive(value) => {
            qualify_type(value, module, binders, local_data, imports, available)?;
        }
        TypeName::Result(ok, err) => {
            qualify_type(ok, module, binders, local_data, imports, available)?;
            qualify_type(err, module, binders, local_data, imports, available)?;
        }
    }
    Ok(())
}

fn qualify_imported_data_name(
    name: &mut String,
    imports: &HashMap<String, ResolvedImport>,
    available: &HashMap<String, String>,
) -> Result<(), String> {
    if let Some((prefix, member)) = name.split_once('.') {
        if let Some(import) = imports.get(prefix) {
            if !import.exports.data.contains(member) {
                return Err(format!("type `{prefix}.{member}` is not public"));
            }
            *name = format!("{}.{member}", import.target);
        } else if available.contains_key(prefix) {
            return Err(format!("module `{prefix}` is not imported"));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn rewrite_body(
    body: &mut Body,
    module: &str,
    root: &str,
    local_data: &HashSet<String>,
    local_functions: &HashSet<String>,
    imports: &HashMap<String, ResolvedImport>,
    available: &HashMap<String, String>,
    results: &HashMap<String, String>,
) -> Result<(), (Span, String)> {
    let rewrite = |expr: &mut Expr| {
        rewrite_expr(
            expr,
            module,
            root,
            local_data,
            local_functions,
            imports,
            available,
            results,
        )
    };
    for statement in &mut body.statements {
        match statement {
            Statement::Yield(_) => {}
            Statement::ReplyVal { .. } => {}
            Statement::ChannelVal {
                capacity,
                message,
                span,
                ..
            } => {
                qualify_type(
                    message,
                    module,
                    &HashSet::new(),
                    local_data,
                    imports,
                    available,
                )
                .map_err(|error| (*span, error))?;
                rewrite(capacity)?;
            }
            Statement::RequestChannelVal { capacity, .. } => rewrite(capacity)?,
            Statement::UnpackVal { value, .. } => rewrite(value)?,
            Statement::Spawn { child, args, .. } => {
                if local_functions.contains(child) {
                    *child = symbol(module, child, root);
                }
                for arg in args {
                    if let CallArgument::Value(expr) = arg {
                        rewrite(expr)?;
                    }
                }
            }
            Statement::Val(_, _, expr)
            | Statement::Var(_, _, expr)
            | Statement::Assign(_, _, expr)
            | Statement::ArrayAssign(_, _, _, expr)
            | Statement::Expr(expr) => rewrite(expr)?,
            Statement::While(condition, body) => {
                rewrite(condition)?;
                rewrite_body(
                    body,
                    module,
                    root,
                    local_data,
                    local_functions,
                    imports,
                    available,
                    results,
                )?;
            }
            Statement::Scope { body, .. } => {
                rewrite_body(
                    body,
                    module,
                    root,
                    local_data,
                    local_functions,
                    imports,
                    available,
                    results,
                )?;
            }
            Statement::With { value, body, .. } => {
                rewrite(value)?;
                rewrite_body(
                    body,
                    module,
                    root,
                    local_data,
                    local_functions,
                    imports,
                    available,
                    results,
                )?;
            }
        }
    }
    if let Some(tail) = &mut body.tail {
        rewrite(tail)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn rewrite_expr(
    expr: &mut Expr,
    module: &str,
    root: &str,
    local_data: &HashSet<String>,
    local_functions: &HashSet<String>,
    imports: &HashMap<String, ResolvedImport>,
    available: &HashMap<String, String>,
    results: &HashMap<String, String>,
) -> Result<(), (Span, String)> {
    let span = expr.span;
    let mut nested = |inner: &mut Expr| {
        rewrite_expr(
            inner,
            module,
            root,
            local_data,
            local_functions,
            imports,
            available,
            results,
        )
    };
    match &mut expr.kind {
        ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Name(_) | ExprKind::Borrow { .. } => {}
        ExprKind::FixedArray(elements) => {
            for element in elements {
                nested(element)?;
            }
        }
        ExprKind::FixedIndex(array, _) => nested(array)?,
        ExprKind::Copy(inner)
        | ExprKind::Neg(inner)
        | ExprKind::Not(inner)
        | ExprKind::Try(inner) => nested(inner)?,
        ExprKind::Add(a, b)
        | ExprKind::Sub(a, b)
        | ExprKind::Mul(a, b)
        | ExprKind::Eq(a, b)
        | ExprKind::Ne(a, b)
        | ExprKind::Lt(a, b)
        | ExprKind::Le(a, b)
        | ExprKind::Gt(a, b)
        | ExprKind::Ge(a, b)
        | ExprKind::And(a, b)
        | ExprKind::Or(a, b) => {
            nested(a)?;
            nested(b)?;
        }
        ExprKind::If(condition, then_body, else_body) => {
            nested(condition)?;
            for body in [then_body, else_body] {
                rewrite_body(
                    body,
                    module,
                    root,
                    local_data,
                    local_functions,
                    imports,
                    available,
                    results,
                )?;
            }
        }
        ExprKind::With { value, body, .. } => {
            nested(value)?;
            rewrite_body(
                body,
                module,
                root,
                local_data,
                local_functions,
                imports,
                available,
                results,
            )?;
        }
        ExprKind::Member(name, _) | ExprKind::VariantCall(name, _, _) => {
            if local_data.contains(name) {
                *name = format!("{module}.{name}");
            } else if let Some(qualified) = results.get(name) {
                *name = qualified.clone();
            } else {
                qualify_imported_data_name(name, imports, available)
                    .map_err(|message| (span, message))?;
            }
            if let ExprKind::VariantCall(_, _, payload) = &mut expr.kind {
                nested(payload)?;
            }
        }
        ExprKind::QualifiedCall(name, member, args) => {
            if local_data.contains(name) {
                *name = format!("{module}.{name}");
            } else if let Some(qualified) = results.get(name) {
                *name = qualified.clone();
            } else if name.contains('.') {
                qualify_imported_data_name(name, imports, available)
                    .map_err(|message| (span, message))?;
            } else if let Some(import) = imports.get(name.as_str()) {
                if !import.exports.functions.contains(member) {
                    return Err((span, format!("function `{name}.{member}` is not public")));
                }
                *name = import.target.clone();
            } else if available.contains_key(name.as_str()) {
                return Err((span, format!("module `{name}` is not imported")));
            }
            rewrite_args(args, &mut nested)?;
        }
        ExprKind::Call(name, args) => {
            if local_functions.contains(name) {
                *name = symbol(module, name, root);
            } else if local_data.contains(name) {
                *name = format!("{module}.{name}");
            } else if let Some(qualified) = results.get(name) {
                *name = qualified.clone();
            }
            rewrite_args(args, &mut nested)?;
        }
        ExprKind::When(subject, arms) => {
            nested(subject)?;
            for arm in arms {
                if local_data.contains(&arm.type_name) {
                    arm.type_name = format!("{module}.{}", arm.type_name);
                } else if let Some(qualified) = results.get(&arm.type_name) {
                    arm.type_name = qualified.clone();
                } else {
                    qualify_imported_data_name(&mut arm.type_name, imports, available)
                        .map_err(|message| (arm.span, message))?;
                }
                rewrite_body(
                    &mut arm.body,
                    module,
                    root,
                    local_data,
                    local_functions,
                    imports,
                    available,
                    results,
                )?;
            }
        }
    }
    Ok(())
}

fn rewrite_args(
    args: &mut [CallArgument],
    nested: &mut impl FnMut(&mut Expr) -> Result<(), (Span, String)>,
) -> Result<(), (Span, String)> {
    for arg in args {
        if let CallArgument::Value(value) = arg {
            nested(value)?;
        }
    }
    Ok(())
}

fn shift_span(span: &mut Span, offset: usize) {
    span.start += offset;
    span.end += offset;
}

fn shift_program(program: &mut syntax::Program, offset: usize) {
    for (ty, span) in &mut program.applied_types {
        let _ = ty;
        shift_span(span, offset);
    }
    for definition in &mut program.data {
        shift_span(&mut definition.span, offset);
        for variant in &mut definition.variants {
            shift_span(&mut variant.span, offset);
            if let Some(payload) = &mut variant.payload {
                shift_span(&mut payload.span, offset);
            }
        }
        for field in &mut definition.fields {
            shift_span(&mut field.span, offset);
        }
    }
    for function in &mut program.functions {
        shift_span(&mut function.name_span, offset);
        for (_, span) in &mut function.type_params {
            shift_span(span, offset);
        }
        for param in &mut function.params {
            shift_span(&mut param.span, offset);
        }
        shift_body(&mut function.body, offset);
    }
}

fn shift_body(body: &mut Body, offset: usize) {
    for statement in &mut body.statements {
        match statement {
            Statement::Yield(span) => shift_span(span, offset),
            Statement::ReplyVal {
                right,
                receiver,
                span,
                ..
            } => {
                shift_span(&mut right.1, offset);
                shift_span(&mut receiver.1, offset);
                shift_span(span, offset);
            }
            Statement::ChannelVal {
                sender,
                receiver,
                capacity,
                span,
                ..
            } => {
                shift_span(&mut sender.1, offset);
                shift_span(&mut receiver.1, offset);
                shift_span(span, offset);
                shift_expr(capacity, offset);
            }
            Statement::RequestChannelVal {
                sender,
                receiver,
                capacity,
                span,
                ..
            } => {
                shift_span(&mut sender.1, offset);
                shift_span(&mut receiver.1, offset);
                shift_span(span, offset);
                shift_expr(capacity, offset);
            }
            Statement::UnpackVal {
                names, value, span, ..
            } => {
                for (_, name_span) in names {
                    shift_span(name_span, offset);
                }
                shift_span(span, offset);
                shift_expr(value, offset);
            }
            Statement::Spawn { args, span, .. } => {
                shift_span(span, offset);
                for arg in args {
                    match arg {
                        CallArgument::Value(expr) => shift_expr(expr, offset),
                        CallArgument::Borrow { span, .. } => shift_span(span, offset),
                    }
                }
            }
            Statement::Val(_, span, value)
            | Statement::Var(_, span, value)
            | Statement::Assign(_, span, value) => {
                shift_span(span, offset);
                shift_expr(value, offset);
            }
            Statement::ArrayAssign(_, _, span, value) => {
                shift_span(span, offset);
                shift_expr(value, offset);
            }
            Statement::While(condition, body) => {
                shift_expr(condition, offset);
                shift_body(body, offset);
            }
            Statement::Scope { span, body } => {
                shift_span(span, offset);
                shift_body(body, offset);
            }
            Statement::With {
                span, value, body, ..
            } => {
                shift_span(span, offset);
                shift_expr(value, offset);
                shift_body(body, offset);
            }
            Statement::Expr(value) => shift_expr(value, offset),
        }
    }
    if let Some(tail) = &mut body.tail {
        shift_expr(tail, offset);
    }
}

fn shift_expr(expr: &mut Expr, offset: usize) {
    shift_span(&mut expr.span, offset);
    match &mut expr.kind {
        ExprKind::Int(_)
        | ExprKind::Bool(_)
        | ExprKind::Name(_)
        | ExprKind::Borrow { .. }
        | ExprKind::Member(_, _) => {}
        ExprKind::FixedArray(elements) => {
            for element in elements {
                shift_expr(element, offset);
            }
        }
        ExprKind::FixedIndex(array, _) => shift_expr(array, offset),
        ExprKind::Copy(value)
        | ExprKind::Neg(value)
        | ExprKind::Not(value)
        | ExprKind::Try(value) => {
            shift_expr(value, offset);
        }
        ExprKind::Add(a, b)
        | ExprKind::Sub(a, b)
        | ExprKind::Mul(a, b)
        | ExprKind::Eq(a, b)
        | ExprKind::Ne(a, b)
        | ExprKind::Lt(a, b)
        | ExprKind::Le(a, b)
        | ExprKind::Gt(a, b)
        | ExprKind::Ge(a, b)
        | ExprKind::And(a, b)
        | ExprKind::Or(a, b) => {
            shift_expr(a, offset);
            shift_expr(b, offset);
        }
        ExprKind::If(condition, then_body, else_body) => {
            shift_expr(condition, offset);
            shift_body(then_body, offset);
            shift_body(else_body, offset);
        }
        ExprKind::With { value, body, .. } => {
            shift_expr(value, offset);
            shift_body(body, offset);
        }
        ExprKind::VariantCall(_, _, payload) => shift_expr(payload, offset),
        ExprKind::QualifiedCall(_, _, args) | ExprKind::Call(_, args) => {
            for arg in args {
                match arg {
                    CallArgument::Value(value) => shift_expr(value, offset),
                    CallArgument::Borrow { span, .. } => shift_span(span, offset),
                }
            }
        }
        ExprKind::When(subject, arms) => {
            shift_expr(subject, offset);
            for arm in arms {
                shift_span(&mut arm.span, offset);
                if let Some((_, span)) = &mut arm.binding {
                    shift_span(span, offset);
                }
                shift_body(&mut arm.body, offset);
            }
        }
    }
}

#[cfg(test)]
mod import_graph_tests {
    use super::*;

    #[test]
    fn long_import_chain_orders_dependencies_without_recursive_stack_use() {
        const COUNT: usize = 8_192;
        let empty = syntax::parse("fn value() -> I64 = 0").unwrap();
        let mut modules = Vec::with_capacity(COUNT);
        for index in 0..COUNT {
            let name = format!("m{index:05}");
            let mut ast = empty.clone();
            let mut available = HashMap::new();
            if index + 1 < COUNT {
                let next = format!("m{:05}", index + 1);
                ast.imports.push(syntax::Import {
                    name: next.clone(),
                    span: Span { start: 0, end: 0 },
                });
                available.insert(next.clone(), next);
            }
            modules.push(Module {
                name,
                package_root: "m00000".into(),
                available,
                path: PathBuf::new(),
                source: String::new(),
                ast,
            });
        }
        let order = check_import_cycles(&modules).unwrap();
        assert_eq!(order.len(), COUNT);
        assert_eq!(order.first().unwrap(), "m08191");
        assert_eq!(order.last().unwrap(), "m00000");

        modules[COUNT - 1].ast.imports.push(syntax::Import {
            name: "m00000".into(),
            span: Span { start: 0, end: 0 },
        });
        modules[COUNT - 1]
            .available
            .insert("m00000".into(), "m00000".into());
        assert!(
            check_import_cycles(&modules)
                .unwrap_err()
                .contains("module import cycle")
        );
    }
}
