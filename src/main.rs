use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use phoebe::{abi, core, frame_layout, generic_core, native, package, project, syntax};

mod build_session_cli;

fn usage() -> &'static str {
    "usage: phoebe lock <project> | phoebe deps <project> | phoebe check <file|project> | phoebe ir <file|project> | phoebe generic-ir <file> | phoebe shapes <file|project> | phoebe frames <file|project> | phoebe clif <file|project> | phoebe cost <file|project> | phoebe run <file|project> [--interp] [function] [I64 args...] | phoebe build <file|project> -o <executable> [--cache-dir <directory>] | phoebe build-session <project> -o <executable>"
}

fn run_arguments(rest: &[String]) -> Result<(String, Vec<i64>), String> {
    let operands = rest
        .iter()
        .filter(|arg| arg.as_str() != "--interp")
        .map(String::as_str)
        .collect::<Vec<_>>();
    let (name, values) = operands
        .split_first()
        .map_or(("main", &[][..]), |(name, rest)| (*name, rest));
    let values = values
        .iter()
        .map(|arg| {
            arg.parse::<i64>()
                .map_err(|_| format!("invalid I64 argument `{arg}`"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((name.to_owned(), values))
}

fn build_arguments(args: impl Iterator<Item = String>) -> Result<(String, Option<String>), String> {
    let mut output = None;
    let mut cache_dir = None;
    let mut args = args;
    while let Some(option) = args.next() {
        let value = args.next().ok_or_else(|| usage().to_owned())?;
        match option.as_str() {
            "-o" if output.is_none() => output = Some(value),
            "--cache-dir" if cache_dir.is_none() => cache_dir = Some(value),
            _ => return Err(usage().into()),
        }
    }
    Ok((output.ok_or_else(|| usage().to_owned())?, cache_dir))
}

fn print_shapes(program: &core::Program) -> Result<(), String> {
    let shapes = abi::Shapes::new(program)?;
    println!("function\tposition\tmode\tparts");
    for function in &program.functions {
        for (index, (&value, mode)) in function.blocks[0]
            .params
            .iter()
            .zip(&function.param_modes)
            .enumerate()
        {
            println!(
                "{}\t{}\t{:?}\t{:?}",
                function.name,
                index,
                mode,
                shapes.parts(function.value_types[value.0])?
            );
        }
        println!(
            "{}\treturn\t-\t{:?}",
            function.name,
            shapes.parts(function.return_type)?
        );
    }
    Ok(())
}

fn print_cost(program: &core::Program) {
    println!("function\tblock\tinstruction\tstart\tend\tkind\ttype\treason");
    for event in program.cost_events() {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            event.function,
            event.block,
            event.instruction,
            event.span.start,
            event.span.end,
            event.kind,
            event.type_name,
            event.reason
        );
    }
}

fn linked_command(
    action: &str,
    args: impl Iterator<Item = String>,
    linked: &core::CheckedLinkedProgram,
) -> Result<(), String> {
    let mut args = args;
    match action {
        "check" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            println!(
                "ok: {} function(s)",
                linked.program().functions.len() + linked.generics().as_module().functions.len()
            );
        }
        "ir" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print!("{}{}", linked.generics().as_module(), linked.program());
        }
        "shapes" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print_shapes(linked.program())?;
        }
        "frames" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print!("{}", frame_layout::format_linked(linked)?);
        }
        "clif" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print!("{}", native::clif_linked(linked)?);
        }
        "cost" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print_cost(linked.program());
        }
        "run" => {
            let rest = args.collect::<Vec<_>>();
            let interpreted = rest.iter().any(|arg| arg == "--interp");
            let (name, values) = run_arguments(&rest)?;
            let result = if interpreted {
                linked.interpret(&name, &values)?
            } else {
                native::run_linked(linked, &name, &values)?
            };
            println!("{result}");
        }
        "build" => {
            let (output, cache_dir) = build_arguments(args)?;
            if cache_dir.is_some() {
                return Err(
                    "linked generic builds do not have a persistent object cache yet".into(),
                );
            }
            native::build_executable_linked(linked, Path::new(&output))?;
            println!("built {output}");
        }
        _ => {
            return Err(format!(
                "{action} for linked generic programs is not available yet"
            ));
        }
    }
    Ok(())
}

fn command() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let action = args.next().ok_or_else(|| usage().to_owned())?;
    if !matches!(
        action.as_str(),
        "lock"
            | "deps"
            | "check"
            | "ir"
            | "generic-ir"
            | "shapes"
            | "frames"
            | "clif"
            | "cost"
            | "run"
            | "build"
            | "build-session"
    ) {
        return Err(usage().to_owned());
    }
    let path = args.next().ok_or_else(|| usage().to_owned())?;
    if matches!(action.as_str(), "lock" | "deps") {
        if args.next().is_some() {
            return Err(usage().into());
        }
        let graph = if action == "lock" {
            package::write_lock(Path::new(&path))?
        } else {
            package::resolve(Path::new(&path))?
        };
        for entry in graph.packages {
            println!("{}\t{}\t{}", entry.locator, entry.name, entry.version);
        }
        return Ok(());
    }
    if action == "build-session" {
        if args.next().as_deref() != Some("-o") {
            return Err(usage().into());
        }
        let output = args.next().ok_or_else(|| usage().to_owned())?;
        if args.next().is_some() {
            return Err(usage().into());
        }
        return build_session_cli::run(Path::new(&path), Path::new(&output));
    }
    if action == "generic-ir" {
        if args.next().is_some() || Path::new(&path).is_dir() || path.ends_with(".toml") {
            return Err(usage().into());
        }
        let source = fs::read_to_string(&path).map_err(|error| format!("{path}: {error}"))?;
        let ast =
            syntax::parse(&source).map_err(|error| format!("{path}:{}", error.render(&source)))?;
        let lowered = generic_core::lower_program(&ast)
            .map_err(|error| format!("{path}:{}", error.render(&source)))?;
        if lowered.functions.is_empty() {
            return Err(format!("{path}: no generic definitions"));
        }
        print!("{lowered}");
        return Ok(());
    }
    let mut args = args.collect::<Vec<_>>().into_iter();
    let mut preloaded = None;
    let mut preloaded_project = None;
    let project_path = Path::new(&path).is_dir() || path.ends_with(".toml");
    if project_path {
        let loaded = project::load(Path::new(&path))?;
        if loaded
            .ast
            .functions
            .iter()
            .any(|function| !function.type_params.is_empty())
        {
            let linked =
                core::lower_linked_checked(&loaded.ast).map_err(|error| loaded.render(&error))?;
            return linked_command(&action, args, &linked);
        }
        preloaded_project = Some(loaded);
    } else {
        let source = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let ast = syntax::parse(&source).map_err(|e| format!("{path}:{}", e.render(&source)))?;
        let interpreted = args.clone().any(|arg| arg == "--interp");
        let has_generics = ast
            .functions
            .iter()
            .any(|function| !function.type_params.is_empty());
        if has_generics || (action == "run" && interpreted) {
            let linked = core::lower_linked_checked(&ast)
                .map_err(|e| format!("{path}:{}", e.render(&source)))?;
            return linked_command(&action, args, &linked);
        }
        preloaded = Some((source, ast));
    }
    let program = if project_path {
        let loaded = preloaded_project.expect("project loaded above");
        core::lower_checked(&loaded.ast).map_err(|error| loaded.render(&error))?
    } else {
        let (source, ast) = if let Some(preloaded) = preloaded {
            preloaded
        } else {
            let source = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
            let ast =
                syntax::parse(&source).map_err(|e| format!("{path}:{}", e.render(&source)))?;
            (source, ast)
        };
        core::lower_checked(&ast).map_err(|e| format!("{path}:{}", e.render(&source)))?
    };
    match action.as_str() {
        "check" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            println!("ok: {} function(s)", program.as_program().functions.len());
        }
        "ir" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print!("{}", program.as_program());
        }
        "shapes" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print_shapes(program.as_program())?;
        }
        "frames" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print!("{}", frame_layout::format(program.as_program())?);
        }
        "clif" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print!("{}", native::clif_checked(&program)?);
        }
        "cost" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print_cost(program.as_program());
        }
        "run" => {
            let rest = args.collect::<Vec<_>>();
            let interpreted = rest.iter().any(|arg| arg == "--interp");
            let (name, values) = run_arguments(&rest)?;
            let result = if interpreted {
                program.interpret(&name, &values)?
            } else {
                native::run_checked(&program, &name, &values)?
            };
            println!("{result}");
        }
        "build" => {
            let (output, cache_dir) = build_arguments(args)?;
            if let Some(directory) = cache_dir {
                let directory = Path::new(&directory);
                let report = native::build_executable_cached_checked(
                    &program,
                    Path::new(&output),
                    directory,
                )?;
                if let Some(error) = report.cache_write_error {
                    eprintln!("native object cache not saved: {error}");
                }
                println!("built {output} (object cache hit: {})", report.reused);
            } else {
                native::build_executable_checked(&program, Path::new(&output))?;
                println!("built {output}");
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn main() -> ExitCode {
    match command() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
