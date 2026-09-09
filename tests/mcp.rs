mod common;
use common::Fixture;
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
};

struct Client {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    id: u32,
}
impl Client {
    async fn start(fixture: &Fixture, fail_build: bool, slow_build: bool) -> Self {
        let mut command = fixture.command();
        command
            .env("MX_TEST_SLOW_BUILD", if slow_build { "1" } else { "" })
            .arg("mcp")
            .env("MX_TEST_FAIL_BUILD", if fail_build { "1" } else { "" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = tokio::process::Command::from(command)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut client = Self {
            child,
            stdin,
            lines,
            id: 0,
        };
        let init = client.request("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"mx-test","version":"1"}})).await;
        assert_eq!(init["result"]["serverInfo"]["name"], "mx");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        client
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        client
    }
    async fn send(&mut self, message: Value) {
        self.stdin
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params}))
            .await;
        loop {
            let line = tokio::time::timeout(Duration::from_secs(15), self.lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .expect("MCP stdout closed");
            let response: Value =
                serde_json::from_str(&line).expect("Non-protocol output on MCP stdout");
            if response.get("id").is_some() {
                assert_eq!(response["id"], self.id);
                return response;
            }
        }
    }
    async fn close(mut self) {
        drop(self.stdin);
        let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success());
    }
}

#[tokio::test]
async fn mcp_discovers_tools_and_runs_without_stdout_pollution() {
    let fixture = Fixture::new();
    let mut client = Client::start(&fixture, false, false).await;
    let tools = client.request("tools/list", json!({})).await;
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 32);
    for name in [
        "mx_devices",
        "mx_inspect",
        "mx_run",
        "mx_status",
        "mx_ui",
        "mx_tap",
        "mx_type",
        "mx_logs",
        "mx_screenshot",
        "mx_capture_flow",
        "mx_relaunch",
        "mx_stop",
        "mx_capacity",
        "mx_fleet",
        "mx_top",
        "mx_profile_catalog",
        "mx_profile_status",
    ] {
        assert!(names.contains(&name));
    }
    let run = client
        .request(
            "tools/call",
            json!({"name":"mx_run","arguments":{"project":fixture.project}}),
        )
        .await;
    assert_ne!(run["result"]["isError"], true, "{run}");
    assert_eq!(run["result"]["structuredContent"]["pid"], 4321);
    assert!(fixture.calls().iter().any(|c| c["args"][1] == "launch"));
    client.close().await;
}

#[tokio::test]
async fn tool_failure_is_recoverable_and_invalid_arguments_do_not_execute() {
    let fixture = Fixture::new();
    let mut client = Client::start(&fixture, true, false).await;
    let invalid = client
        .request(
            "tools/call",
            json!({"name":"mx_run","arguments":{"project":fixture.project,"unexpected":true}}),
        )
        .await;
    assert!(invalid.get("error").is_some() || invalid["result"]["isError"] == true);
    assert!(fixture.calls().is_empty());
    let failed = client
        .request(
            "tools/call",
            json!({"name":"mx_run","arguments":{"project":fixture.project}}),
        )
        .await;
    assert_eq!(failed["result"]["isError"], true);
    assert!(
        failed["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Build failed")
    );
    let devices = client
        .request("tools/call", json!({"name":"mx_devices","arguments":{}}))
        .await;
    assert_ne!(devices["result"]["isError"], true);
    client.close().await;
}

#[tokio::test]
async fn cancellation_stops_build_and_releases_the_operation_lock() {
    let fixture = Fixture::new();
    let mut client = Client::start(&fixture, false, true).await;
    client.id += 1;
    let run_id = client.id;
    client.send(json!({"jsonrpc":"2.0","id":run_id,"method":"tools/call","params":{"name":"mx_run","arguments":{"project":fixture.project}}})).await;
    let pid_path = fixture.dir.path().join("build.pid");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !pid_path.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":run_id,"reason":"test cancellation"}})).await;
    // The SDK suppresses responses to cancelled requests. Verify the actual process stops.
    let pid = std::fs::read_to_string(pid_path).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let running = std::process::Command::new("/bin/kill")
                .args(["-0", &pid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            if !running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let inspect = client
        .request(
            "tools/call",
            json!({"name":"mx_inspect","arguments":{"project":fixture.project}}),
        )
        .await;
    assert_ne!(inspect["result"]["isError"], true);
    assert!(!fixture.calls().iter().any(|c| c["args"][1] == "install"));
    client.close().await;
}

#[tokio::test]
async fn inline_images_and_live_log_cursors_work() {
    let fixture = Fixture::new();
    let mut client = Client::start(&fixture, false, false).await;
    let run = client
        .request(
            "tools/call",
            json!({"name":"mx_run","arguments":{"project":fixture.project}}),
        )
        .await;
    assert_ne!(run["result"]["isError"], true);
    let image=client.request("tools/call",json!({"name":"mx_screenshot","arguments":{"device":"test-device","output":fixture.dir.path().join("mcp.png")}})).await;
    assert_eq!(image["result"]["content"][1]["type"], "image");
    let started = client
        .request(
            "tools/call",
            json!({"name":"mx_logs_start","arguments":{"device":"test-device","pid":4321}}),
        )
        .await;
    let id = started["result"]["structuredContent"]["stream_id"]
        .as_str()
        .unwrap();
    let mut cursor = 0;
    let mut total = 0;
    for _ in 0..30 {
        let batch = client
            .request(
                "tools/call",
                json!({"name":"mx_logs_read","arguments":{"stream_id":id,"after":cursor}}),
            )
            .await;
        let batch = &batch["result"]["structuredContent"];
        let entries = batch["entries"].as_array().unwrap();
        for entry in entries {
            assert!(entry["sequence"].as_u64().unwrap() > cursor);
        }
        cursor = batch["cursor"].as_u64().unwrap();
        total += entries.len();
        if batch["done"] == true && entries.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(total, 250);
    client
        .request(
            "tools/call",
            json!({"name":"mx_logs_stop","arguments":{"stream_id":id}}),
        )
        .await;
    client.close().await;
}

#[tokio::test]
async fn another_clients_run_cannot_silently_rebind_input() {
    let fixture = Fixture::new();
    let mut first = Client::start(&fixture, false, false).await;
    let mut second = Client::start(&fixture, false, false).await;
    first
        .request(
            "tools/call",
            json!({"name":"mx_run","arguments":{"project":fixture.project}}),
        )
        .await;
    second
        .request(
            "tools/call",
            json!({"name":"mx_run","arguments":{"project":fixture.project}}),
        )
        .await;
    let refused=first.request("tools/call",json!({"name":"mx_tap","arguments":{"device":"test-device","selector":{"identifier":"name"}}})).await;
    assert_eq!(refused["result"]["isError"], true);
    assert!(
        refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("replaced")
    );
    assert!(!fixture.calls().iter().any(|c| c["args"][0] == "tap"));
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn build_failure_exposes_structured_diagnostics() {
    let fixture = Fixture::new();
    let mut client = Client::start(&fixture, true, false).await;
    let failed = client
        .request(
            "tools/call",
            json!({"name":"mx_run","arguments":{"project":fixture.project}}),
        )
        .await;
    assert_eq!(
        failed["result"]["structuredContent"]["diagnostics"][0]["line"],
        7
    );
    assert_eq!(
        failed["result"]["structuredContent"]["diagnostics"][0]["severity"],
        "error"
    );
    client.close().await;
}
