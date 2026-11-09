use phoebe::{core, native, syntax};

#[test]
fn nested_closed_field_reads_preserve_the_parent_and_each_other() {
    let source = "data Inner { n: I64 } data Outer { left: Inner, right: Inner }
fn main() -> I64 {
  val outer = Outer(Inner(20), Inner(1));
  val first = outer.left;
  val second = outer.left;
  val right = outer.right;
  val third = outer.left;
  first.n + second.n + right.n + third.n - 19
}";
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    assert_eq!(
        program
            .cost_events()
            .iter()
            .filter(|event| event.reason == "inferred field value")
            .count(),
        4
    );
}

#[test]
fn nested_field_read_from_var_survives_parent_replacement() {
    let source = "data Inner { n: I64 } data Outer { inner: Inner }
fn main() -> I64 {
  var outer = Outer(Inner(20));
  val old = outer.inner;
  outer = Outer(Inner(22));
  val next = outer.inner;
  old.n + next.n
}";
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
}

#[test]
fn closed_sibling_can_be_read_from_resource_record_before_right_is_closed() {
    let source = "data Inner { n: I64 } data Packet { meta: Inner, sender: Sender<I64> }
fn acquire() -> Result<I64, ResourceError> {
  val (sender, receiver) = channel<I64>(capacity = 1)?;
  val packet = Packet(Inner(20), sender);
  val first = packet.meta;
  val second = packet.meta;
  packet.close_caps();
  receiver.close();
  Result<I64, ResourceError>.Ok(first.n + second.n + 2)
}
fn main() -> I64 = when (acquire()) {
  Result<I64, ResourceError>.Ok(value) -> { value },
  Result<I64, ResourceError>.Err(error) -> { 0 },
}";
    for source in [
        source.to_owned(),
        source.replace("val packet =", "var packet ="),
    ] {
        let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    }
    let invalid = source.replace(
        "val first = packet.meta;",
        "val moved = packet.sender;\n  moved.close();\n  val first = packet.meta;",
    );
    let error = core::lower(&syntax::parse(&invalid).unwrap()).unwrap_err();
    assert!(error.message.contains("not live"), "{error:?}");
}
