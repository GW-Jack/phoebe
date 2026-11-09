use phoebe::{core, syntax};

fn checked(body: &str) -> core::CheckedProgram {
    core::lower_checked(&syntax::parse(body).unwrap()).unwrap()
}

fn words() -> String {
    std::iter::once("40")
        .chain(std::iter::repeat_n("0", 254))
        .chain(std::iter::once("2"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn sharing_requires_a_large_array_copy_site() {
    let copied = checked(&format!(
        "fn main() -> I64 {{ var a = [{}]; val prior = a; prior[0] + a[255] }}",
        words()
    ));
    assert!(copied.storage_plan().shares_array(256));
    assert!(!copied.storage_plan().shares_array(2));

    let one_use = checked(&format!(
        "fn main() -> I64 {{ val a = [{}]; a[0] + a[255] }}",
        words()
    ));
    assert!(!one_use.storage_plan().has_shared_arrays());
    assert!(one_use.as_program().cost_events().is_empty());

    let explicit_only = checked(&format!(
        "fn main() -> I64 {{ val a = [{}]; val prior = copy(a); prior[0] + a[255] }}",
        words()
    ));
    assert!(!explicit_only.storage_plan().has_shared_arrays());
    assert!(
        explicit_only
            .as_program()
            .cost_events()
            .iter()
            .any(|event| event.kind == "copy.value")
    );
}

#[test]
fn retained_and_embedded_arrays_use_inline_fallback() {
    let retained = checked(&format!(
        "fn main() -> I64 {{ var a = [{}]; val prior = a; yield; prior[0] + a[255] }}",
        words()
    ));
    assert!(!retained.storage_plan().has_shared_arrays());

    let embedded = checked(&format!(
        "data Box {{ words: [I64; 256] }}\nfn main() -> I64 {{ var a = [{}]; val prior = a; prior[0] + a[255] }}",
        words()
    ));
    assert!(!embedded.storage_plan().has_shared_arrays());
}

#[test]
fn selected_storage_reports_material_sites() {
    let program = checked(&format!(
        "fn main() -> I64 {{ var a = [{}]; val prior = a; a[0] = 0; prior[0] + a[255] }}",
        words()
    ));
    let events = program.as_program().cost_events();
    for kind in [
        "storage.allocate",
        "storage.retain",
        "storage.cow",
        "storage.release",
    ] {
        assert!(
            events.iter().any(|event| event.kind == kind),
            "missing {kind}"
        );
    }
    assert!(!events.iter().any(|event| event.kind == "copy.value"));
    assert!(events.iter().all(|event| event.span.end > event.span.start));
}
