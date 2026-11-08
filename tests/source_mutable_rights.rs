use phoebe::{core, native, runtime::ScopeLimits, syntax};

#[test]
fn mutable_pair_bindings_lower_directly_into_owned_places() {
    for source in [
        include_str!("../examples/channel_exchange.phoebe").replace(
            "val (sender, receiver) = channel<I64>",
            "var (sender, receiver) = channel<I64>",
        ),
        include_str!("../examples/reply_exchange.phoebe").replace(
            "val (right, receiver) = reply<I64>",
            "var (right, receiver) = reply<I64>",
        ),
        include_str!("../examples/request_backpressure.phoebe")
            .replace(
                "val (sender, receiver) = request_channel<I64>",
                "var (sender, receiver) = request_channel<I64>",
            )
            .replace(
                "val (value, right) = incoming.unpack();",
                "var (value, right) = incoming.unpack();",
            ),
        include_str!("../examples/product_unpack.phoebe")
            .replace("val (pair, bonus)", "var (pair, bonus)")
            .replace("val (left, right)", "var (left, right)"),
    ] {
        let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
        assert!(program.to_string().contains("place.init"));
        assert_eq!(program.interpret("main", &[]).unwrap(), 42);
        assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    }
}

#[test]
fn mutable_pair_reassignment_requires_the_old_right_to_be_empty() {
    let source = include_str!("../examples/channel_exchange.phoebe").replace(
        "val (sender, receiver) = channel<I64>(capacity = 1)?;",
        "var (sender, receiver) = channel<I64>(capacity = 1)?; \
         val (second, other) = channel<I64>(capacity = 1)?; sender = second; \
         other.close();",
    );
    assert!(core::lower(&syntax::parse(&source).unwrap()).is_err());
}

#[test]
fn mutable_sender_moves_reassigns_and_closes_across_yield() {
    let source = include_str!("../examples/mutable_channel_right.phoebe");
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert!(program.to_string().contains("place.take"));
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);

    // Failure of the second creation must close the first sender held in a
    // place and its receiver before returning the typed resource error.
    let limited = ScopeLimits {
        channels: 1,
        ..ScopeLimits::default()
    };
    assert_eq!(
        program.interpret_with_limits("main", &[], limited).unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&program, "main", &[], limited).unwrap(),
        0
    );

    let directory = std::env::temp_dir().join(format!(
        "phoebe-mutable-right-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let executable = directory.join("program");
    native::build_executable(&program, &executable).unwrap();
    let result = std::process::Command::new(&executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"42\n");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn mutable_right_cannot_be_taken_twice_or_overwritten_live() {
    let twice = "fn main() -> Result<I64, ResourceError> {\n\
        val (sender, receiver) = channel<I64>(capacity = 1)?;\n\
        var active = sender;\n\
        val first = active;\n\
        val second = active;\n\
        first.close(); second.close(); receiver.close();\n\
        Result<I64, ResourceError>.Ok(42)\n\
    }";
    let error = core::lower(&syntax::parse(twice).unwrap()).unwrap_err();
    assert!(error.message.contains("use after move"), "{error:?}");
    assert_eq!(
        error.span.start,
        twice.find("val second = active;").unwrap() + "val second = ".len()
    );

    let overwrite = "fn main() -> Result<I64, ResourceError> {\n\
        val (first, first_receiver) = channel<I64>(capacity = 1)?;\n\
        val (second, second_receiver) = channel<I64>(capacity = 1)?;\n\
        var active = first;\n\
        active = second;\n\
        active.close(); first_receiver.close(); second_receiver.close();\n\
        Result<I64, ResourceError>.Ok(42)\n\
    }";
    assert!(core::lower(&syntax::parse(overwrite).unwrap()).is_err());
}

#[test]
fn forked_and_reply_rights_can_live_in_mutable_places() {
    let main = "\nfn main() -> I64 = when (use_right()) {\n\
        Result<I64, ResourceError>.Ok(value) -> { value },\n\
        Result<I64, ResourceError>.Err(error) -> { 0 }\n\
    }";
    let fork = "fn use_right() -> Result<I64, ResourceError> {\n\
        val (sender, receiver) = channel<I64>(capacity = 1)?;\n\
        var source = sender;\n\
        var extra = source.fork()?;\n\
        yield; extra.close(); source.close(); receiver.close();\n\
        Result<I64, ResourceError>.Ok(42)\n\
    }";
    let fork = core::lower(&syntax::parse(&format!("{fork}{main}")).unwrap()).unwrap();
    assert_eq!(fork.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&fork, "main", &[]).unwrap(), 42);
    let no_extra_handles = ScopeLimits {
        handles: 2,
        ..ScopeLimits::default()
    };
    assert_eq!(
        fork.interpret_with_limits("main", &[], no_extra_handles)
            .unwrap(),
        0
    );
    assert_eq!(
        native::run_with_limits(&fork, "main", &[], no_extra_handles).unwrap(),
        0
    );

    let reply = "fn use_right() -> Result<I64, ResourceError> {\n\
        val (right, receiver) = reply<I64>()?;\n\
        var active = right;\n\
        yield; active.close(); receiver.close();\n\
        Result<I64, ResourceError>.Ok(42)\n\
    }";
    let reply = core::lower(&syntax::parse(&format!("{reply}{main}")).unwrap()).unwrap();
    assert_eq!(reply.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&reply, "main", &[]).unwrap(), 42);
}

#[test]
fn completing_a_mutable_reply_consumes_its_right() {
    let source = "fn use_right() -> Result<I64, ResourceError> {\n\
        val (right, receiver) = reply<I64>()?;\n\
        var active = right;\n\
        val completed = active.complete(42);\n\
        val answer = receiver.receive();\n\
        when (answer) {\n\
          Reply<I64>.Value(value) -> { Result<I64, ResourceError>.Ok(value) },\n\
          Reply<I64>.End -> { Result<I64, ResourceError>.Ok(0) }\n\
        }\n\
    }\n\
    fn main() -> I64 = when (use_right()) {\n\
        Result<I64, ResourceError>.Ok(value) -> { value },\n\
        Result<I64, ResourceError>.Err(error) -> { 0 }\n\
    }";
    let program = core::lower(&syntax::parse(source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);
    let duplicate = source.replace(
        "val answer = receiver.receive();",
        "active.close(); val answer = receiver.receive();",
    );
    assert!(core::lower(&syntax::parse(&duplicate).unwrap()).is_err());
}

#[test]
fn mutable_sender_retries_after_full_queue() {
    let source = include_str!("../examples/channel_exchange.phoebe")
        .replace(
            "val first = sender.send(20);",
            "var active = sender; val first = active.send(20);",
        )
        .replace("sender.send(22)", "active.send(22)")
        .replace("sender.close();", "active.close();");
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);

    let directory = std::env::temp_dir().join(format!(
        "phoebe-mutable-send-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let executable = directory.join("program");
    native::build_executable(&program, &executable).unwrap();
    let result = std::process::Command::new(&executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"42\n");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn mutable_request_endpoints_survive_pending_traffic() {
    let source = include_str!("../examples/request_backpressure.phoebe")
        .replace(
            "fn service(receiver: RequestReceiver<I64>) -> I64 {",
            "fn service(receiver: RequestReceiver<I64>) -> I64 { var active = receiver;",
        )
        .replace("receiver.receive()", "active.receive()")
        .replace("receiver.close();", "active.close();")
        .replace(
            "spawn service(receiver)?;",
            "spawn service(receiver)?; var active = sender;",
        )
        .replace("sender.send(", "active.send(")
        .replace("sender.close();", "active.close();");
    let program = core::lower(&syntax::parse(&source).unwrap()).unwrap();
    assert_eq!(program.interpret("main", &[]).unwrap(), 42);
    assert_eq!(native::run(&program, "main", &[]).unwrap(), 42);

    let directory = std::env::temp_dir().join(format!(
        "phoebe-mutable-request-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let executable = directory.join("program");
    native::build_executable(&program, &executable).unwrap();
    let result = std::process::Command::new(&executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"42\n");
    std::fs::remove_dir_all(directory).unwrap();
}
