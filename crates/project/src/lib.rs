//! Manifest loading and module name resolution for a local Phoebe package.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use phoebe_syntax::syntax::{self, Body, CallArgument, Expr, ExprKind, Statement, TypeName};
use phoebe_syntax::{Diagnostic, Span};

struct SourceFile {
    path: PathBuf,
    start: usize,
    source: String,
}

pub struct Loaded {
    pub ast: syntax::Program,
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

struct Module {
    name: String,
    path: PathBuf,
    source: String,
    ast: syntax::Program,
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
    let manifest_path = if path.is_dir() {
        path.join("phoebe.toml")
    } else {
        path.to_owned()
    };
    let source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let manifest: toml::Table =
        toml::from_str(&source).map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let table = &manifest;
    if table.contains_key("dependencies") {
        return Err("external dependencies are not implemented in this project loader".into());
    }
    let package = table
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or("manifest requires [package]")?;
    let name = manifest_string(package, "name")?;
    if !identifier(name) {
        return Err("package name must be an identifier".into());
    }
    let root = manifest_string(package, "root")?;
    let modules = table
        .get("modules")
        .and_then(toml::Value::as_table)
        .ok_or("manifest requires [modules]")?;
    if !modules.contains_key(root) {
        return Err(format!("root module `{root}` is not listed in [modules]"));
    }
    let directory = manifest_path.parent().unwrap_or(Path::new("."));
    let mut modules = modules
        .iter()
        .map(|(name, path)| {
            if !identifier(name) {
                return Err(format!("invalid module name `{name}`"));
            }
            let relative = path
                .as_str()
                .ok_or_else(|| format!("module `{name}` path must be a string"))?;
            let path = directory.join(relative);
            let source = fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let ast = syntax::parse(&source)
                .map_err(|error| format!("{}:{}", path.display(), error.render(&source)))?;
            Ok(Module {
                name: name.clone(),
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
    for module in &modules {
        let mut seen = HashSet::new();
        for import in &module.ast.imports {
            if !names.contains(import.name.as_str()) {
                return Err(format!(
                    "{}:{}",
                    module.path.display(),
                    Diagnostic::new(import.span, format!("unknown module {}", import.name))
                        .render(&module.source)
                ));
            }
            if import.name == module.name || import.name == root || !seen.insert(&import.name) {
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

    let mut combined = syntax::Program {
        imports: Vec::new(),
        data: Vec::new(),
        functions: Vec::new(),
        applied_results: Vec::new(),
        uses_send_outcome: false,
        uses_receive_outcome: false,
    };
    let mut sources = Vec::new();
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
            .map(|item| item.name.clone())
            .collect::<HashSet<_>>();
        if let Some(collision) = imports.iter().find(|name| local_data.contains(*name)) {
            return Err(format!(
                "{}: imported module `{collision}` conflicts with a local data type",
                module.path.display()
            ));
        }
        let mut result_names = HashMap::new();
        for (ty, span) in &mut module.ast.applied_results {
            let before = ty.key();
            qualify_type(ty, &module.name, &local_data, &imports, &names)
                .map_err(|message| source_error(path, source, *span, message))?;
            result_names.insert(before, ty.key());
        }
        for definition in &mut module.ast.data {
            for variant in &mut definition.variants {
                if let Some(payload) = &mut variant.payload {
                    qualify_type(&mut payload.ty, &module.name, &local_data, &imports, &names)
                        .map_err(|message| source_error(path, source, payload.span, message))?;
                }
            }
            for field in &mut definition.fields {
                qualify_type(&mut field.ty, &module.name, &local_data, &imports, &names)
                    .map_err(|message| source_error(path, source, field.span, message))?;
            }
            definition.name = format!("{}.{}", module.name, definition.name);
        }
        for function in &mut module.ast.functions {
            for param in &mut function.params {
                qualify_type(&mut param.ty, &module.name, &local_data, &imports, &names)
                    .map_err(|message| source_error(path, source, param.span, message))?;
            }
            qualify_type(
                &mut function.return_type,
                &module.name,
                &local_data,
                &imports,
                &names,
            )
            .map_err(|message| source_error(path, source, function.name_span, message))?;
            rewrite_body(
                &mut function.body,
                &module.name,
                root,
                &local_data,
                &local_functions,
                &imports,
                &names,
                &result_names,
            )
            .map_err(|(span, message)| source_error(path, source, span, message))?;
            function.name = symbol(&module.name, &function.name, root);
        }
        shift_program(&mut module.ast, offset);
        combined.data.extend(module.ast.data);
        combined.functions.extend(module.ast.functions);
        combined.applied_results.extend(module.ast.applied_results);
        combined.uses_send_outcome |= module.ast.uses_send_outcome;
        combined.uses_receive_outcome |= module.ast.uses_receive_outcome;
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
    Ok(Loaded {
        ast: combined,
        sources,
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
                    .map(|import| import.name.as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    fn visit<'a>(
        name: &'a str,
        graph: &BTreeMap<&'a str, Vec<&'a str>>,
        visiting: &mut HashSet<&'a str>,
        done: &mut HashSet<&'a str>,
        order: &mut Vec<String>,
    ) -> Result<(), String> {
        if done.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name) {
            return Err(format!("module import cycle includes `{name}`"));
        }
        for import in &graph[name] {
            visit(import, graph, visiting, done, order)?;
        }
        visiting.remove(name);
        done.insert(name);
        order.push(name.to_owned());
        Ok(())
    }
    let mut visiting = HashSet::new();
    let mut done = HashSet::new();
    let mut order = Vec::new();
    for name in graph.keys() {
        visit(name, &graph, &mut visiting, &mut done, &mut order)?;
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
    local_data: &HashSet<String>,
    imports: &HashSet<String>,
    modules: &HashSet<String>,
) -> Result<(), String> {
    match ty {
        TypeName::I64 | TypeName::Bool | TypeName::SenderI64 | TypeName::ReceiverI64 => {}
        TypeName::Named(name) if local_data.contains(name) => {
            *name = format!("{module}.{name}");
        }
        TypeName::Named(name) => {
            if let Some((prefix, _)) = name.split_once('.')
                && modules.contains(prefix)
                && !imports.contains(prefix)
            {
                return Err(format!("module `{prefix}` is not imported"));
            }
        }
        TypeName::Result(ok, err) => {
            qualify_type(ok, module, local_data, imports, modules)?;
            qualify_type(err, module, local_data, imports, modules)?;
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
    imports: &HashSet<String>,
    modules: &HashSet<String>,
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
            modules,
            results,
        )
    };
    for statement in &mut body.statements {
        match statement {
            Statement::ChannelVal { capacity, .. } => rewrite(capacity)?,
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
                    modules,
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
    imports: &HashSet<String>,
    modules: &HashSet<String>,
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
            modules,
            results,
        )
    };
    match &mut expr.kind {
        ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Name(_) => {}
        ExprKind::Copy(inner) | ExprKind::Neg(inner) | ExprKind::Try(inner) => nested(inner)?,
        ExprKind::Add(a, b)
        | ExprKind::Sub(a, b)
        | ExprKind::Mul(a, b)
        | ExprKind::Eq(a, b)
        | ExprKind::Lt(a, b) => {
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
                    modules,
                    results,
                )?;
            }
        }
        ExprKind::Member(name, _) | ExprKind::VariantCall(name, _, _) => {
            if local_data.contains(name) {
                *name = format!("{module}.{name}");
            } else if let Some(qualified) = results.get(name) {
                *name = qualified.clone();
            }
            if let ExprKind::VariantCall(_, _, payload) = &mut expr.kind {
                nested(payload)?;
            }
        }
        ExprKind::QualifiedCall(name, _member, args) => {
            if local_data.contains(name) {
                *name = format!("{module}.{name}");
            } else if let Some(qualified) = results.get(name) {
                *name = qualified.clone();
            } else if modules.contains(name.as_str()) && !imports.contains(name) {
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
                }
                rewrite_body(
                    &mut arm.body,
                    module,
                    root,
                    local_data,
                    local_functions,
                    imports,
                    modules,
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
    for (ty, span) in &mut program.applied_results {
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
        for param in &mut function.params {
            shift_span(&mut param.span, offset);
        }
        shift_body(&mut function.body, offset);
    }
}

fn shift_body(body: &mut Body, offset: usize) {
    for statement in &mut body.statements {
        match statement {
            Statement::ChannelVal {
                sender,
                receiver,
                capacity,
                span,
            } => {
                shift_span(&mut sender.1, offset);
                shift_span(&mut receiver.1, offset);
                shift_span(span, offset);
                shift_expr(capacity, offset);
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
            Statement::While(condition, body) => {
                shift_expr(condition, offset);
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
        ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Name(_) | ExprKind::Member(_, _) => {}
        ExprKind::Copy(value) | ExprKind::Neg(value) | ExprKind::Try(value) => {
            shift_expr(value, offset);
        }
        ExprKind::Add(a, b)
        | ExprKind::Sub(a, b)
        | ExprKind::Mul(a, b)
        | ExprKind::Eq(a, b)
        | ExprKind::Lt(a, b) => {
            shift_expr(a, offset);
            shift_expr(b, offset);
        }
        ExprKind::If(condition, then_body, else_body) => {
            shift_expr(condition, offset);
            shift_body(then_body, offset);
            shift_body(else_body, offset);
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
