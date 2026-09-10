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
        "doctor",
        "devices",
        "inspect",
        "run",
        "logs",
        "screenshot",
        "capture-flow",
        "relaunch",
        "stop",
        "web",
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

#[test]
fn mcp_config_uses_absolute_executable_and_only_explicit_overrides() {
    let result = Command::new(env!("CARGO_BIN_EXE_mx"))
        .arg("mcp-config")
        .env_remove("MX_AXE_PATH")
        .env_remove("MX_STATE_DIR")
        .output()
        .unwrap();
    assert!(result.status.success());
    let config: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let server = &config["mcpServers"]["mx"];
    assert!(std::path::Path::new(server["command"].as_str().unwrap()).is_absolute());
    assert_eq!(server["args"], serde_json::json!(["mcp"]));
    assert!(server.get("env").is_none());

    let result = Command::new(env!("CARGO_BIN_EXE_mx"))
        .arg("mcp-config")
        .env("MX_AXE_PATH", "/custom path/axe")
        .env("MX_STATE_DIR", "/custom path/state")
        .output()
        .unwrap();
    assert!(result.status.success());
    let config: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        config["mcpServers"]["mx"]["env"]["MX_AXE_PATH"],
        "/custom path/axe"
    );
    assert_eq!(
        config["mcpServers"]["mx"]["env"]["MX_STATE_DIR"],
        "/custom path/state"
    );
}
