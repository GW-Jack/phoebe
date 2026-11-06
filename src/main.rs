use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use phoebe::{core, native, syntax};

fn usage() -> &'static str {
    "usage: phoebe check <file> | phoebe ir <file> | phoebe run <file> [--interp] [function] [I64 args...] | phoebe build <file> -o <executable>"
}

fn command() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let action = args.next().ok_or_else(|| usage().to_owned())?;
    if !matches!(action.as_str(), "check" | "ir" | "run" | "build") {
        return Err(usage().to_owned());
    }
    let path = args.next().ok_or_else(|| usage().to_owned())?;
    let source = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let ast = syntax::parse(&source).map_err(|e| format!("{path}:{}", e.render(&source)))?;
    let program = core::lower(&ast).map_err(|e| format!("{path}:{}", e.render(&source)))?;
    match action.as_str() {
        "check" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            println!("ok: {} function(s)", program.functions.len());
        }
        "ir" => {
            if args.next().is_some() {
                return Err(usage().into());
            }
            print!("{program}");
        }
        "run" => {
            let rest = args.collect::<Vec<_>>();
            let interpreted = rest.iter().any(|arg| arg == "--interp");
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
            let result = if interpreted {
                program.interpret(name, &values)?
            } else {
                native::run(&program, name, &values)?
            };
            println!("{result}");
        }
        "build" => {
            if args.next().as_deref() != Some("-o") {
                return Err(usage().into());
            }
            let output = args.next().ok_or_else(|| usage().to_owned())?;
            if args.next().is_some() {
                return Err(usage().into());
            }
            native::build_executable(&program, Path::new(&output))?;
            println!("built {output}");
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
