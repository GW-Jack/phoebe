//! Compare the selected managed ABI with the inline fallback on the same
//! reachable Phoebe functions. The unreachable `force_inline` function carries
//! the array through a retained frame, so the planner chooses the inline
//! fallback for the baseline.

use phoebe::{core, native, syntax};
use std::hint::black_box;
use std::time::Instant;

fn run(len: usize) {
    let words = std::iter::once("x")
        .chain(std::iter::repeat_n("0", len - 2))
        .chain(std::iter::once("2"))
        .collect::<Vec<_>>()
        .join(", ");
    let calls = std::iter::repeat_n("sum(a)", 10)
        .collect::<Vec<_>>()
        .join(" + ");
    let source = format!(
        "fn sum(xs: [I64; {len}]) -> I64 = xs[0] + xs[{}]\n\
         fn main(x: I64) -> I64 {{ val a = [{words}]; {calls} }}",
        len - 1
    );
    let managed = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    let inline = core::lower(
        &syntax::parse(&format!(
            "{source}\nfn force_inline(xs: [I64; {len}]) -> I64 {{ yield; xs[0] }}"
        ))
        .unwrap(),
    )
    .unwrap();
    assert!(core::StoragePlan::for_program(&managed).shares_array(len));
    assert!(!core::StoragePlan::for_program(&inline).has_shared_arrays());
    let managed = native::compile_jit(&managed, "main").unwrap();
    let inline = native::compile_jit(&inline, "main").unwrap();
    for executable in [&managed, &inline] {
        assert_eq!(executable.run(&[40]).unwrap(), 420);
    }
    for (label, executable) in [
        ("managed", &managed),
        ("inline", &inline),
        ("inline", &inline),
        ("managed", &managed),
    ] {
        let now = Instant::now();
        let mut sum = 0i64;
        for _ in 0..200_000 {
            sum += black_box(executable.run(black_box(&[40])).unwrap());
        }
        println!("{len} words {label} {:?} checksum={sum}", now.elapsed());
    }
}

fn main() {
    run(256);
}
