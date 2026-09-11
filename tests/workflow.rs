mod common;
use common::Fixture;
use serde_json::Value;

#[test]
fn full_run_preserves_paths_and_orders_boot_install_launch() {
    let fixture = Fixture::new();
    let result = fixture
        .command()
        .arg("run")
        .arg("--project")
        .arg(&fixture.project)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["bundle_id"], "dev.mx.test");
    assert_eq!(result["pid"], 4321);
    let calls = fixture.calls();
    let build = calls
        .iter()
        .find(|c| c["args"].as_array().unwrap().last() == Some(&Value::String("build".into())))
        .unwrap();
    assert_eq!(
        build["args"][1],
        fixture
            .project
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    let ops: Vec<_> = calls
        .iter()
        .filter(|c| c["program"] == "xcrun")
        .map(|c| c["args"][1].as_str().unwrap())
        .collect();
    assert_eq!(
        ops,
        ["list", "list", "boot", "bootstatus", "install", "launch"]
    );
}

#[test]
fn build_failure_retains_diagnostics_and_never_boots_or_installs() {
    let fixture = Fixture::new();
    let result = fixture
        .command()
        .env("MX_TEST_FAIL_BUILD", "1")
        .arg("run")
        .arg("--project")
        .arg(&fixture.project)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Build failed"));
    assert!(
        std::fs::read_to_string(fixture.dir.path().join(".mx/test-device/build.log"))
            .unwrap()
            .contains("intentional build failure")
    );
    assert!(!fixture.calls().iter().any(|c| {
        ["boot", "install", "launch"]
            .iter()
            .any(|op| c["args"][1] == *op)
    }));
}

#[test]
fn ambiguous_tap_does_not_send_input() {
    let fixture = Fixture::new();
    let result = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args(["tap", "--device", "test-device", "--label", "Continue"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("matched 2 elements"));
    assert!(!fixture.calls().iter().any(|c| c["args"][0] == "tap"));
}

#[test]
fn unique_tap_and_literal_text_are_forwarded_without_shell_parsing() {
    let fixture = Fixture::new();
    let result = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args(["tap", "--device", "test-device", "--id", "name"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = "--literal $(no-command) `no-command` \"quoted\"\n";
    let result = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args(["type", "--device", "test-device", text])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("typed.txt")).unwrap(),
        text
    );
    assert!(fixture.calls().iter().any(|c| {
        c["args"]
            .as_array()
            .unwrap()
            .contains(&Value::String("--id=name".into()))
    }));
}

#[test]
fn recent_logs_are_bounded_and_report_truncation() {
    let fixture = Fixture::new();
    let result = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args([
            "logs",
            "--device",
            "test-device",
            "--pid",
            "4321",
            "--last",
            "30",
        ])
        .output()
        .unwrap();
    assert!(result.status.success());
    let result: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["truncated"], true);
    assert_eq!(result["logs"].as_str().unwrap().lines().count(), 200);
}

#[test]
fn screenshot_refuses_to_replace_an_existing_file() {
    let fixture = Fixture::new();
    let path = fixture.dir.path().join("existing.png");
    std::fs::write(&path, b"original").unwrap();
    let result = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args(["screenshot", "--device", "test-device"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert!(!fixture.calls().iter().any(|c| c["args"][1] == "io"));
}

#[test]
fn screenshot_captures_to_a_new_path_and_publishes_without_temporary_files() {
    let fixture = Fixture::new();
    let destination = fixture.dir.path().join("image with spaces.png");
    let result = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args(["screenshot", "--device", "test-device"])
        .arg(&destination)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"\x89PNG\r\n\x1a\n");
    let calls = fixture.calls();
    let capture = calls.iter().find(|c| c["args"][1] == "io").unwrap();
    let temporary = capture["args"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap();
    assert!(!std::path::Path::new(temporary).exists());
    let response: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        response["path"],
        destination
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
}

#[test]
fn screenshot_chooses_a_new_output_path_when_omitted() {
    let fixture = Fixture::new();
    let result = fixture
        .command()
        .current_dir(fixture.dir.path())
        .env("MX_TEST_BOOTED", "1")
        .args(["screenshot", "--device", "test-device"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: Value = serde_json::from_slice(&result.stdout).unwrap();
    let path = std::path::Path::new(result["path"].as_str().unwrap());
    assert!(
        path.starts_with(
            fixture
                .dir
                .path()
                .canonicalize()
                .unwrap()
                .join(".mx/screenshots")
        )
    );
    assert_eq!(std::fs::read(path).unwrap(), b"\x89PNG\r\n\x1a\n");
}

#[test]
fn capture_flow_writes_named_screenshots_and_semantic_manifest() {
    let fixture = Fixture::new();
    let plan = fixture.dir.path().join("flow-plan.json");
    let output = fixture.dir.path().join("flow-output");
    std::fs::write(
        &plan,
        serde_json::to_vec(&serde_json::json!({
            "device": "test-device",
            "output_dir": output,
            "states": [
                {"name": "welcome", "actions": [], "expect_label": "Name"},
                {"name": "continued", "actions": [
                    {"action": "tap", "selector": {"identifier": "first"}}
                ], "expect_label": "Name"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let result = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args(["capture-flow"])
        .arg(&plan)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["state_count"], 2);
    assert!(output.join("01-welcome.png").exists());
    assert!(output.join("02-continued.png").exists());
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(output.join("flow.json")).unwrap()).unwrap();
    assert_eq!(manifest["states"][0]["name"], "welcome");
    assert_eq!(
        manifest["states"][1]["screen"]["elements"][0]["label"],
        "Continue"
    );
}

#[test]
fn concurrent_cli_run_is_rejected_and_ctrl_c_stops_the_first_build() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let mut first = fixture
        .command()
        .env("MX_TEST_SLOW_BUILD", "1")
        .arg("run")
        .arg("--project")
        .arg(&fixture.project)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while !fixture.dir.path().join("build.pid").exists() {
        if started.elapsed() > Duration::from_secs(5) {
            let _ = first.kill();
            panic!("First build did not start");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let second = fixture
        .command()
        .arg("run")
        .arg("--project")
        .arg(&fixture.project)
        .output()
        .unwrap();
    let signal = std::process::Command::new("/bin/kill")
        .args(["-INT", &first.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success());
    let status = first.wait().unwrap();
    assert_eq!(status.code(), Some(130));
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("Device is busy"));
}

#[test]
fn wrong_foreground_pid_prevents_tap_and_typing() {
    let fixture = Fixture::new();
    for arguments in [
        vec!["tap", "--device", "test-device", "--id", "name"],
        vec!["type", "--device", "test-device", "secret"],
    ] {
        let output = fixture
            .command()
            .env("MX_TEST_BOOTED", "1")
            .env("MX_TEST_FOREGROUND_PID", "9999")
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Foreground application changed"));
    }
    assert!(
        !fixture
            .calls()
            .iter()
            .any(|c| c["args"][0] == "tap" || c["args"][0] == "type" || c["args"][1] == "pbcopy")
    );
}
#[test]
fn unicode_uses_simulator_pasteboard_and_paste_key() {
    let fixture = Fixture::new();
    let text = "José 東京 🧉";
    let output = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args(["type", "--device", "test-device", text])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("typed.txt")).unwrap(),
        text
    );
    assert!(fixture.calls().iter().any(|c| c["args"][1] == "pbcopy"));
}
#[test]
fn observations_persist_across_processes_and_unchanged_ui_is_a_delta() {
    let fixture = Fixture::new();
    let first = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args(["observe", "--device", "test-device"])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    let revision = first["revision"].to_string();
    let next = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args(["observe", "--device", "test-device", "--since", &revision])
        .output()
        .unwrap();
    assert!(next.status.success());
    let next: Value = serde_json::from_slice(&next.stdout).unwrap();
    assert!(next["full"].is_null());
    assert_eq!(next["changed"], serde_json::json!([]));
    assert_eq!(next["revision"], first["revision"]);
}

#[test]
fn stale_reference_and_unmet_expectation_are_errors() {
    let fixture = Fixture::new();
    let observed = fixture
        .command()
        .args(["observe", "--device", "test-device"])
        .output()
        .unwrap();
    assert!(observed.status.success());
    for (actions, expected, message) in [
        (
            serde_json::json!([{"action":"tap_reference","reference":1,"revision":999}]),
            None,
            "Stale UI revision",
        ),
        (
            serde_json::json!([{"action":"tap","selector":{"identifier":"name"}}]),
            Some("never appears"),
            "UI did not settle",
        ),
    ] {
        let path = fixture.dir.path().join("action.json");
        std::fs::write(&path, serde_json::to_vec(&serde_json::json!({"device":"test-device","actions":actions,"expect_label":expected,"timeout_ms":500})).unwrap()).unwrap();
        let output = fixture.command().arg("action").arg(path).output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn fleet_mutations_reject_unowned_devices() {
    let fixture = Fixture::new();
    for operation in ["boot", "shutdown", "delete"] {
        let output = fixture
            .command()
            .args([operation, "--device", "test-device"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            !fixture
                .calls()
                .iter()
                .any(|c| c["program"] == "xcrun" && c["args"][1] == operation)
        );
    }
}

#[test]
fn interrupted_profile_restores_only_changes_and_preserves_existing_overrides() {
    let fixture = Fixture::new();
    let state = fixture.dir.path().join("state");
    let session_path = state.join("test-device.session.json");
    let mut session: Value =
        serde_json::from_slice(&std::fs::read(&session_path).unwrap()).unwrap();
    session["active"] = false.into();
    std::fs::write(session_path, serde_json::to_vec(&session).unwrap()).unwrap();
    std::fs::write(
        state.join("test-device.owned.json"),
        r#"{"device":"test-device","source":"source","name":"Mx test"}"#,
    )
    .unwrap();
    std::fs::write(
        fixture.dir.path().join("disabled.json"),
        r#"["com.apple.assetsd"]"#,
    )
    .unwrap();
    let request = fixture.dir.path().join("profile.json");
    std::fs::write(
        &request,
        r#"{"device":"test-device","operation":"apply","preset":"balanced"}"#,
    )
    .unwrap();
    let failed = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .env("MX_TEST_PROFILE_FAIL", "1")
        .arg("profile")
        .arg(&request)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(state.join("test-device.profile.json").exists());
    std::fs::write(
        &request,
        r#"{"device":"test-device","operation":"restore"}"#,
    )
    .unwrap();
    let restored = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .arg("profile")
        .arg(&request)
        .output()
        .unwrap();
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert!(!state.join("test-device.profile.json").exists());
    let disabled: Value =
        serde_json::from_slice(&std::fs::read(fixture.dir.path().join("disabled.json")).unwrap())
            .unwrap();
    assert_eq!(disabled, serde_json::json!(["com.apple.assetsd"]));
}

#[test]
fn slim_profile_runs_optional_post_boot_cleanup() {
    let fixture = Fixture::new();
    let state = fixture.dir.path().join("state");
    let session_path = state.join("test-device.session.json");
    let mut session: Value =
        serde_json::from_slice(&std::fs::read(&session_path).unwrap()).unwrap();
    session["active"] = false.into();
    std::fs::write(session_path, serde_json::to_vec(&session).unwrap()).unwrap();
    std::fs::write(
        state.join("test-device.owned.json"),
        r#"{"device":"test-device","source":"source","name":"Mx test"}"#,
    )
    .unwrap();
    let request = fixture.dir.path().join("profile.json");
    std::fs::write(
        &request,
        r#"{"device":"test-device","operation":"apply","preset":"slim"}"#,
    )
    .unwrap();

    let output = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .arg("profile")
        .arg(&request)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fixture.calls();
    assert!(calls.iter().any(|call| {
        call["program"] == "xcrun"
            && call["args"]
                == serde_json::json!(["simctl", "terminate", "test-device", "com.apple.Spotlight"])
    }));
    assert!(calls.iter().any(|call| {
        call["program"] == "xcrun"
            && call["args"].as_array().is_some_and(|args| {
                args.iter().any(|arg| {
                    arg.as_str().is_some_and(|value| {
                        value.contains("com.apple.nanotimekitcompaniond")
                            && value.contains("com.apple.nanoappregistryd")
                            && value.contains("com.apple.nanomapscd")
                            && value.contains("com.apple.nanoprefsyncd.2")
                            && !value.contains(") &")
                    })
                })
            })
    }));
    assert!(calls.iter().any(|call| {
        call["program"] == "xcrun"
            && call["args"].as_array().is_some_and(|args| {
                args.iter().any(|arg| {
                    arg.as_str().is_some_and(|value| {
                        value.contains("com.apple.biomed")
                            && value.contains("com.apple.managedconfiguration.profiled")
                    })
                })
            })
    }));
}

#[test]
fn prebuilt_launch_reuses_app_without_invoking_xcode_again() {
    let fixture = Fixture::new();
    let run = fixture
        .command()
        .arg("run")
        .arg("--project")
        .arg(&fixture.project)
        .output()
        .unwrap();
    assert!(run.status.success());
    let run: Value = serde_json::from_slice(&run.stdout).unwrap();
    let before = fixture
        .calls()
        .iter()
        .filter(|c| c["program"] == "xcodebuild")
        .count();
    let launched = fixture
        .command()
        .args([
            "launch",
            "--device",
            "test-device",
            "--app",
            run["app"].as_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        launched.status.success(),
        "{}",
        String::from_utf8_lossy(&launched.stderr)
    );
    let launched: Value = serde_json::from_slice(&launched.stdout).unwrap();
    assert_eq!(launched["built"], false);
    assert_ne!(launched["session_id"], run["session_id"]);
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|c| c["program"] == "xcodebuild")
            .count(),
        before
    );
}

#[test]
fn relaunch_skips_build_install_and_device_discovery() {
    let fixture = Fixture::new();
    let before = fixture.calls().len();
    let launched = fixture
        .command()
        .args(["relaunch", "--device", "test-device"])
        .output()
        .unwrap();
    assert!(
        launched.status.success(),
        "{}",
        String::from_utf8_lossy(&launched.stderr)
    );
    let calls = fixture.calls();
    let new_calls = &calls[before..];
    assert_eq!(new_calls.len(), 1);
    assert_eq!(new_calls[0]["args"][1], "launch");
}

#[test]
fn fresh_device_creation_uses_requested_type_without_cloning_source_data() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args([
            "create",
            "--template",
            "test-device",
            "--name",
            "Compact",
            "--device-type",
            "com.apple.CoreSimulator.SimDeviceType.iPhone-SE-3rd-generation",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fixture.calls();
    let created = calls.iter().find(|c| c["args"][1] == "create").unwrap();
    assert_eq!(
        created["args"][3],
        "com.apple.CoreSimulator.SimDeviceType.iPhone-SE-3rd-generation"
    );
    assert!(!calls.iter().any(|c| c["args"][1] == "clone"));
    assert!(
        fixture
            .dir
            .path()
            .join("state/00000000-0000-4000-8000-000000000001.owned.json")
            .exists()
    );
}
#[test]
fn env_gated_watch_mode_settles_from_a_single_axe_process() {
    let fixture = Fixture::new();
    let plan = fixture.dir.path().join("action.json");
    std::fs::write(
        &plan,
        serde_json::to_vec(&serde_json::json!({"device":"test-device","actions":[{"action":"tap","selector":{"identifier":"first"}}],"timeout_ms":2000})).unwrap(),
    )
    .unwrap();
    let result = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .env("MX_UI_WATCH_MIN_VERSION", "1.0.0")
        .arg("action")
        .arg(plan)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let axe_calls: Vec<_> = fixture
        .calls()
        .into_iter()
        .filter(|c| c["program"] == "axe")
        .collect();
    let watch_calls = axe_calls
        .iter()
        .filter(|c| {
            c["args"][0] == "describe-ui"
                && c["args"].as_array().unwrap().iter().any(|a| a == "--watch")
        })
        .count();
    let poll_calls = axe_calls
        .iter()
        .filter(|c| {
            c["args"][0] == "describe-ui"
                && !c["args"].as_array().unwrap().iter().any(|a| a == "--watch")
        })
        .count();
    assert!(watch_calls >= 2, "{axe_calls:?}");
    assert!(poll_calls <= 1, "{axe_calls:?}");
}
