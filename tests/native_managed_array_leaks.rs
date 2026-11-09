#![cfg(debug_assertions)]

use phoebe::{core, native, runtime, syntax};

fn words() -> String {
    std::iter::once("40")
        .chain(std::iter::repeat_n("0", 254))
        .chain(std::iter::once("2"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn direct_managed_arrays_release_on_call_branch_and_return() {
    let baseline = runtime::debug_live_word_allocations();
    let source = format!(
        "fn edit(xs: [I64; 256]) -> I64 {{ var local = xs; local[0] = 0; local[255] }}\n\
         fn main() -> I64 {{ var a = [{}]; val prior = a; \
         val branch = if (true) {{ edit(a) }} else {{ 0 }}; branch + prior[0] }}",
        words()
    );
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    assert!(core::StoragePlan::for_program(&program).shares_array(256));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);

    let returned = format!(
        "fn id(xs: [I64; 256]) -> [I64; 256] = xs\n\
         fn main() -> I64 {{ var a = [{}]; val prior = a; id(a)[255] + prior[0] }}",
        words()
    );
    let returned = core::lower(&syntax::parse(&returned).unwrap()).unwrap();
    assert!(core::StoragePlan::for_program(&returned).shares_array(256));
    assert_eq!(returned.interpret("main", &[]).unwrap(), 42);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);
    assert_eq!(native::run(&returned, "main", &[]).unwrap(), 42);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);

    let read_only = format!(
        "fn sum(xs: [I64; 256]) -> I64 = xs[0] + xs[255]\n\
         fn main() -> I64 {{ val a = [{}]; sum(a) + sum(a) }}",
        words()
    );
    let read_only = core::lower(&syntax::parse(&read_only).unwrap()).unwrap();
    let plan = core::StoragePlan::for_program(&read_only);
    assert!(plan.borrows_param(0, 0));
    assert!(plan.shares_array(256));
    assert!(
        read_only
            .cost_events()
            .iter()
            .any(|event| event.kind == "storage.borrow")
    );
    assert!(
        !read_only
            .cost_events()
            .iter()
            .any(|event| event.kind == "storage.retain")
    );
    let retains = runtime::debug_word_retain_calls();
    assert_eq!(read_only.interpret("main", &[]).unwrap(), 84);
    assert_eq!(runtime::debug_word_retain_calls(), retains);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);
    assert_eq!(native::run(&read_only, "main", &[]).unwrap(), 84);
    assert_eq!(runtime::debug_word_retain_calls(), retains);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);

    let explicit = format!(
        "fn main() -> I64 {{ var a = [{}]; val shared = a; val prior = copy(a); a[0] = 0; shared[0] + prior[0] + a[255] }}",
        words()
    );
    let explicit = core::lower(&syntax::parse(&explicit).unwrap()).unwrap();
    assert!(core::StoragePlan::for_program(&explicit).shares_array(256));
    assert!(
        explicit
            .cost_events()
            .iter()
            .any(|event| event.kind == "storage.copy_words")
    );
    let retains = runtime::debug_word_retain_calls();
    assert_eq!(explicit.interpret("main", &[]).unwrap(), 82);
    assert_eq!(runtime::debug_word_retain_calls(), retains + 1);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);
    assert_eq!(native::run(&explicit, "main", &[]).unwrap(), 82);
    assert_eq!(runtime::debug_word_retain_calls(), retains + 2);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);

    let replacement = format!(
        "fn main() -> I64 {{ var a = [{}]; val prior = a; val other = a; a = [{}]; prior[0] + other[0] + a[255] }}",
        words(),
        words()
    );
    let replacement = core::lower(&syntax::parse(&replacement).unwrap()).unwrap();
    assert!(core::StoragePlan::for_program(&replacement).shares_array(256));
    assert_eq!(replacement.interpret("main", &[]).unwrap(), 82);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);
    assert_eq!(native::run(&replacement, "main", &[]).unwrap(), 82);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);

    let joined = format!(
        "fn main() -> I64 {{ val a = [{}]; val prior = a; \
         val picked = if (true) {{ a }} else {{ a }}; prior[0] + picked[255] }}",
        words()
    );
    let joined = core::lower(&syntax::parse(&joined).unwrap()).unwrap();
    assert!(core::StoragePlan::for_program(&joined).shares_array(256));
    assert_eq!(joined.interpret("main", &[]).unwrap(), 42);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);
    assert_eq!(native::run(&joined, "main", &[]).unwrap(), 42);
    assert_eq!(runtime::debug_live_word_allocations(), baseline);
}
