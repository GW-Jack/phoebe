//! Developer benchmark for frontend, JIT, and repeated native invocation.
//! Run with `cargo run --release --example bench -- examples/hello.phoebe 30`.

use std::env;
use std::fs;
use std::time::{Duration, Instant};

use phoebe::{core, native, syntax};

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let path = args.next().ok_or("usage: bench <file.phoebe> [rounds]")?;
    let rounds = args
        .next()
        .map(|value| value.parse::<usize>().map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or(30);
    if rounds == 0 || args.next().is_some() {
        return Err("usage: bench <file.phoebe> [rounds > 0]".into());
    }
    let source = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let mut parse_times = Vec::with_capacity(rounds);
    let mut lower_times = Vec::with_capacity(rounds);
    let mut jit_times = Vec::with_capacity(rounds);
    let mut invoke_times = Vec::with_capacity(rounds);
    let mut answer = None;
    for _ in 0..rounds {
        let started = Instant::now();
        let ast = syntax::parse(&source).map_err(|error| error.render(&source))?;
        parse_times.push(started.elapsed());
        let started = Instant::now();
        let program = core::lower(&ast).map_err(|error| error.render(&source))?;
        lower_times.push(started.elapsed());
        let started = Instant::now();
        let executable = native::compile_jit(&program, "main")?;
        jit_times.push(started.elapsed());
        let started = Instant::now();
        let mut value = 0;
        for _ in 0..1000 {
            value = std::hint::black_box(executable.run(&[])?);
        }
        invoke_times.push(started.elapsed());
        if let Some(prior) = answer
            && prior != value
        {
            return Err("nondeterministic benchmark result".into());
        }
        answer = Some(value);
    }
    println!("file: {path}");
    println!("host: {}-{}", env::consts::OS, env::consts::ARCH);
    println!("rounds: {rounds}");
    println!("result: {}", answer.expect("at least one round"));
    println!(
        "median parse: {:.3} ms",
        median(parse_times).as_secs_f64() * 1000.0
    );
    println!(
        "median lower+verify: {:.3} ms",
        median(lower_times).as_secs_f64() * 1000.0
    );
    println!(
        "median JIT compile: {:.3} ms",
        median(jit_times).as_secs_f64() * 1000.0
    );
    println!(
        "median wrapper+native invocation: {:.1} ns/call (1000 calls per round)",
        median(invoke_times).as_secs_f64() * 1_000_000.0
    );
    Ok(())
}
