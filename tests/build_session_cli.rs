use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn cli_session_reuses_a_warm_project_without_restarting() {
    let directory =
        std::env::temp_dir().join(format!("phoebe-build-session-cli-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for (name, source) in [
        (
            "phoebe.toml",
            include_str!("../examples/multi_module/phoebe.toml"),
        ),
        (
            "app.phoebe",
            include_str!("../examples/multi_module/app.phoebe"),
        ),
        (
            "math.phoebe",
            include_str!("../examples/multi_module/math.phoebe"),
        ),
    ] {
        std::fs::write(directory.join(name), source).unwrap();
    }
    let executable = directory.join("app");
    let compiler = std::env::var_os("PHOEBE_TEST_COMPILER")
        .or_else(|| option_env!("CARGO_BIN_EXE_phoebe").map(Into::into))
        .expect("test compiler path");
    let mut child = Command::new(compiler)
        .arg("build-session")
        .arg(&directory)
        .arg("-o")
        .arg(&executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"build\nbuild\nquit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 4, "{stdout}");
    assert_eq!(lines[0], "ready");
    assert!(lines[1].contains("parsed=2\tchanged_interfaces=2\treused_core=0"));
    assert!(lines[1].contains("linked=true"));
    assert!(lines[2].contains("parsed=0\tchanged_interfaces=0\treused_core=6"));
    assert!(lines[2].contains("linked=false"));
    let reused_objects = lines[2]
        .split('\t')
        .find_map(|field| field.strip_prefix("reused_objects="))
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert!(reused_objects > 0);
    assert!(lines[2].contains("elapsed_ms="));
    assert_eq!(lines[3], "bye");
    let app = Command::new(&executable).output().unwrap();
    assert!(app.status.success());
    assert_eq!(app.stdout, b"42\n");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cli_session_reuses_a_warm_generic_project() {
    let directory = std::env::temp_dir().join(format!(
        "phoebe-build-session-generic-cli-{}",
        std::process::id()
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
    let executable = directory.join("app");
    let compiler = std::env::var_os("PHOEBE_TEST_COMPILER")
        .or_else(|| option_env!("CARGO_BIN_EXE_phoebe").map(Into::into))
        .expect("test compiler path");
    let mut child = Command::new(compiler)
        .arg("build-session")
        .arg(&directory)
        .arg("-o")
        .arg(&executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"build\nbuild\nquit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 4, "{stdout}");
    assert!(lines[1].contains("parsed=3	changed_interfaces=3	reused_core=0"));
    assert!(lines[1].contains("reused_objects=0	linked=true"));
    assert!(lines[2].contains("parsed=0	changed_interfaces=0	reused_core=1"));
    assert!(lines[2].contains("reused_objects=1	linked=false"));
    assert_eq!(lines[3], "bye");
    assert_eq!(Command::new(&executable).output().unwrap().stdout, b"42\n");
    std::fs::remove_dir_all(directory).unwrap();
}
