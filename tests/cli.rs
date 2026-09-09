use std::process::Command;

#[test]
fn help_exposes_implemented_commands() {
    let result = Command::new(env!("CARGO_BIN_EXE_mx"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(result.status.success());
    let help = String::from_utf8(result.stdout).unwrap();
    for command in [
        "devices",
        "inspect",
        "run",
        "logs",
        "screenshot",
        "capture-flow",
        "relaunch",
        "stop",
    ] {
        assert!(help.contains(command));
    }
}

#[test]
fn missing_project_fails_without_success_json() {
    let result = Command::new(env!("CARGO_BIN_EXE_mx"))
        .args(["run", "--project", "/nonexistent-mx-test-project"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Project path does not exist"));
}

#[test]
fn logs_require_a_numeric_pid() {
    let result = Command::new(env!("CARGO_BIN_EXE_mx"))
        .args(["logs", "--device", "test", "--pid", "predicate injection"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
}
