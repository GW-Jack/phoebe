//! A line-oriented trigger for the in-process incremental project builder.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::Instant;

use phoebe::ProjectBuildSession;

pub(crate) fn run(project: &Path, output: &Path) -> Result<(), String> {
    let mut session = ProjectBuildSession::new();
    let input = io::stdin();
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "ready").map_err(|error| error.to_string())?;
    stdout.flush().map_err(|error| error.to_string())?;
    for line in input.lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        match line.trim() {
            "build" => {
                let started = Instant::now();
                match session.build(project, output) {
                    Ok(report) => {
                        writeln!(
                        stdout,
                        "built\t{}\tparsed={}\tchanged_interfaces={}\treused_core={}\treused_objects={}\telapsed_ms={:.3}",
                        output.display(),
                        report.compile.load.parsed.len(),
                        report.compile.load.changed_interfaces.len(),
                        report.compile.reused_core_functions,
                        report.reused_native_objects,
                        started.elapsed().as_secs_f64() * 1000.0,
                    )
                    .map_err(|error| error.to_string())?;
                    }
                    Err(error) => {
                        writeln!(stdout, "error\t{}", error.replace(['\t', '\n', '\r'], " "))
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            "quit" => {
                writeln!(stdout, "bye").map_err(|error| error.to_string())?;
                stdout.flush().map_err(|error| error.to_string())?;
                break;
            }
            _ => {
                writeln!(stdout, "error\texpected `build` or `quit`")
                    .map_err(|error| error.to_string())?;
            }
        }
        stdout.flush().map_err(|error| error.to_string())?;
    }
    Ok(())
}
