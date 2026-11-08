use std::path::PathBuf;
use std::process::Command;

#[test]
fn generic_source_runs_and_builds_from_the_cli() {
    let compiler = std::env::var_os("PHOEBE_TEST_COMPILER")
        .map(PathBuf::from)
        .or_else(|| option_env!("CARGO_BIN_EXE_phoebe").map(PathBuf::from))
        .expect("compiler path supplied by Cargo or Buck2");
    let directory = std::env::temp_dir().join(format!(
        "phoebe-generic-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("program.phoebe");
    let executable = directory.join("program");
    std::fs::write(&source, include_str!("../examples/generic_call.phoebe")).unwrap();

    for suffix in [None, Some("--interp")] {
        let mut run = Command::new(&compiler);
        run.arg("run").arg(&source);
        if let Some(suffix) = suffix {
            run.arg(suffix);
        }
        let result = run.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, b"42\n");
    }

    let build = Command::new(&compiler)
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let result = Command::new(&executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"42\n");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn generic_project_resolves_public_calls_across_modules() {
    let compiler = std::env::var_os("PHOEBE_TEST_COMPILER")
        .map(PathBuf::from)
        .or_else(|| option_env!("CARGO_BIN_EXE_phoebe").map(PathBuf::from))
        .expect("compiler path supplied by Cargo or Buck2");
    let directory = std::env::temp_dir().join(format!(
        "phoebe-generic-project-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    for (name, source) in [
        (
            "phoebe.toml",
            include_str!("../examples/generic_project/phoebe.toml"),
        ),
        (
            "app.phoebe",
            include_str!("../examples/generic_project/app.phoebe"),
        ),
        (
            "math.phoebe",
            include_str!("../examples/generic_project/math.phoebe"),
        ),
        (
            "base.phoebe",
            include_str!("../examples/generic_project/base.phoebe"),
        ),
    ] {
        std::fs::write(directory.join(name), source).unwrap();
    }
    for suffix in [None, Some("--interp")] {
        let mut run = Command::new(&compiler);
        run.arg("run").arg(&directory);
        if let Some(suffix) = suffix {
            run.arg(suffix);
        }
        let result = run.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, b"42\n");
    }
    let executable = directory.join("program");
    let build = Command::new(&compiler)
        .arg("build")
        .arg(&directory)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let result = Command::new(&executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"42\n");

    let cache = directory.join("native-cache");
    for expected_hit in [false, true] {
        let cached = Command::new(&compiler)
            .arg("build")
            .arg(&directory)
            .arg("-o")
            .arg(&executable)
            .arg("--cache-dir")
            .arg(&cache)
            .output()
            .unwrap();
        assert!(
            cached.status.success(),
            "{}",
            String::from_utf8_lossy(&cached.stderr)
        );
        assert!(
            String::from_utf8_lossy(&cached.stdout)
                .contains(&format!("object cache hit: {expected_hit}"))
        );
    }

    for (action, expected) in [
        ("check", "ok: 3 function(s)"),
        ("ir", "call.generic"),
        ("clif", "Phoebe generic instance"),
        ("shapes", "main\treturn"),
        ("frames", "no retained frames"),
        ("cost", "function\tblock"),
    ] {
        let result = Command::new(&compiler)
            .arg(action)
            .arg(&directory)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{action}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains(expected));
    }

    std::fs::write(
        directory.join("base.phoebe"),
        "pub data T { value: I64 }\npub fn identity<T>(value: T) -> T = value",
    )
    .unwrap();
    let shadowed = Command::new(&compiler)
        .arg("run")
        .arg(&directory)
        .output()
        .unwrap();
    assert!(
        shadowed.status.success(),
        "{}",
        String::from_utf8_lossy(&shadowed.stderr)
    );
    assert_eq!(shadowed.stdout, b"42\n");

    std::fs::write(
        directory.join("base.phoebe"),
        "fn identity<T>(value: T) -> T = value",
    )
    .unwrap();
    let hidden = Command::new(&compiler)
        .arg("check")
        .arg(&directory)
        .output()
        .unwrap();
    assert!(!hidden.status.success());
    assert!(String::from_utf8_lossy(&hidden.stderr).contains("not public"));
    std::fs::remove_dir_all(directory).unwrap();
}
