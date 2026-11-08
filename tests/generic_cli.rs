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
