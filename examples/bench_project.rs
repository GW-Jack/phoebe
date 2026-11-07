//! Measure a warm in-process project session against fresh project compilation.
//! Run with `cargo run --release --example bench_project -- examples/multi_module 100`.

use std::env;
use std::path::Path;
use std::time::{Duration, Instant};

use phoebe::{ProjectCompilerSession, core, project};

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: bench_project <project> [rounds]")?;
    let rounds = args
        .next()
        .map(|value| value.parse::<usize>().map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or(100);
    if rounds == 0 || args.next().is_some() {
        return Err("usage: bench_project <project> [rounds > 0]".into());
    }
    let path = Path::new(&path);
    let mut session = ProjectCompilerSession::new();
    session.compile(path)?;
    let mut fresh_times = Vec::with_capacity(rounds);
    let mut warm_times = Vec::with_capacity(rounds);
    let mut reused = None;
    for _ in 0..rounds {
        let started = Instant::now();
        let loaded = project::load(path)?;
        let fresh = core::lower(&loaded.ast).map_err(|error| loaded.render(&error))?;
        fresh_times.push(started.elapsed());

        let started = Instant::now();
        let (warm, report) = session.compile(path)?;
        warm_times.push(started.elapsed());
        if fresh.to_string() != warm.to_string() {
            return Err("warm Core differs from fresh Core".into());
        }
        reused = Some(report.reused_core_functions);
    }
    println!("host: {}-{}", env::consts::OS, env::consts::ARCH);
    println!("rounds: {rounds}");
    println!("reused Core functions: {}", reused.unwrap_or(0));
    println!(
        "median fresh load+lower: {:.3} ms",
        median(fresh_times).as_secs_f64() * 1000.0
    );
    println!(
        "median warm session: {:.3} ms",
        median(warm_times).as_secs_f64() * 1000.0
    );
    Ok(())
}
