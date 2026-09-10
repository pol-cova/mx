use anyhow::Context;
use mx::{fleet, interaction, logs, runtime, session, sim, ui};
use rmcp::{RoleServer, service::RequestContext};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    tool, tool_handler, tool_router,
};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Default)]
pub struct Server {
    logs: logs::Manager,
    bindings: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, (String, String)>>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectRequest {
    pub project: PathBuf,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceRequest {
    pub device: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopRequest {
    pub device: String,
    pub bundle_id: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotRequest {
    pub device: String,
    pub output: Option<PathBuf>,
    #[serde(default = "default_inline")]
    pub inline: bool,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TapRequest {
    pub device: String,
    pub selector: ui::Selector,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeRequest {
    pub device: String,
    pub text: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogsRequest {
    pub device: String,
    /// Application process ID returned by mx_run.
    pub pid: u32,
    /// Look back 1 to 300 seconds. Defaults to 30. Returns at most 200 lines / 64 KiB.
    #[serde(default = "default_seconds")]
    pub seconds: u32,
}
fn default_inline() -> bool {
    true
}
fn default_seconds() -> u32 {
    30
}

fn result<T: serde::Serialize>(value: anyhow::Result<T>) -> CallToolResult {
    match value.and_then(|v| Ok(serde_json::to_value(v)?)) {
        Ok(value) => CallToolResult::structured(value),
        Err(error) => match error.downcast_ref::<mx::diagnostics::BuildFailure>() {
            Some(failure) => serde_json::to_value(failure).map_or_else(
                |serialization| {
                    CallToolResult::error(vec![ContentBlock::text(serialization.to_string())])
                },
                CallToolResult::structured_error,
            ),
            None => CallToolResult::error(vec![ContentBlock::text(format!("{error:#}"))]),
        },
    }
}

async fn cancellable<T: serde::Serialize>(
    context: RequestContext<RoleServer>,
    operation: impl std::future::Future<Output = anyhow::Result<T>>,
) -> CallToolResult {
    tokio::select! {
        _ = context.ct.cancelled() => CallToolResult::error(vec![ContentBlock::text("Operation cancelled")]),
        value = operation => result(value),
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveRequest {
    pub device: String,
    pub since: Option<u64>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LiveRequest {
    pub device: String,
    pub pid: u32,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StreamRequest {
    pub stream_id: String,
    #[serde(default)]
    pub after: u64,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloneRequest {
    pub source: String,
    pub name: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRequest {
    pub template: String,
    pub name: String,
    pub device_type: Option<String>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BootRequest {
    pub device: String,
    #[serde(default = "boot_limit")]
    pub max_booted: usize,
    pub memory_budget: Option<sim::MemoryBudget>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopRequest {
    pub device: String,
    #[serde(default = "top_limit")]
    pub limit: usize,
}
fn top_limit() -> usize {
    12
}
fn boot_limit() -> usize {
    8
}

impl Server {
    fn bound(&self, device: &str) -> anyhow::Result<String> {
        let bindings = self.bindings.lock().unwrap();
        let (canonical, expected) = bindings.get(device).ok_or_else(|| {
            anyhow::anyhow!(
                "This MCP client has not bound the device; call mx_run or mx_use_session first"
            )
        })?;
        let current = session::active(canonical)?;
        anyhow::ensure!(
            &current.id == expected,
            "Another client replaced this app session; refusing to target its app"
        );
        Ok(expected.clone())
    }
    fn remember(&self, device: &str, alias: Option<&str>, id: &str) {
        let mut bindings = self.bindings.lock().unwrap();
        bindings.insert(device.into(), (device.into(), id.into()));
        if let Some(alias) = alias {
            bindings.insert(alias.into(), (device.into(), id.into()));
        }
    }
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UseSession {
    pub device: String,
    pub session_id: String,
}
#[tool_router]
impl Server {
    #[tool(
        description = "List available iOS simulators with UDIDs, runtimes, and boot state.",
        annotations(read_only_hint = true)
    )]
    async fn mx_devices(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        cancellable(context, async {
            sim::list()
                .await
                .map(|devices| serde_json::json!({"devices": devices}))
        })
        .await
    }

    #[tool(
        description = "Discover an Xcode project or workspace and list its schemes.",
        annotations(read_only_hint = true)
    )]
    async fn mx_inspect(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<ProjectRequest>,
    ) -> CallToolResult {
        cancellable(context, async { runtime::inspect(request.project).await }).await
    }

    #[tool(
        description = "Build, boot, install, and launch an iOS app. Returns bundle ID, PID, device, timing, and build log path. The build can take up to 10 minutes."
    )]
    async fn mx_run(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<runtime::RunRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let alias = request.device.clone();
            let result = runtime::run(request).await?;
            self.remember(&result.device, alias.as_deref(), &result.session_id);
            Ok(result)
        })
        .await
    }

    #[tool(
        description = "Install and launch a prebuilt simulator .app, binding a session without running Xcode. Build once with mx_run, then reuse its app path across compatible simulators."
    )]
    async fn mx_launch(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<runtime::LaunchRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let alias = r.device.clone();
            let value = runtime::launch(r).await?;
            let device = value["device"]
                .as_str()
                .context("Launch result omitted device")?;
            let session_id = value["session_id"]
                .as_str()
                .context("Launch result omitted session ID")?;
            self.remember(device, Some(&alias), session_id);
            Ok(value)
        })
        .await
    }
    #[tool(
        description = "Relaunch the app already installed for an existing Mx session without reinstalling or rebuilding it. Use for warm journey retries and branch captures."
    )]
    async fn mx_relaunch(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<runtime::RelaunchRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let alias = r.device.clone();
            let value = runtime::relaunch(r).await?;
            let device = value["device"]
                .as_str()
                .context("Relaunch result omitted device")?;
            let session_id = value["session_id"]
                .as_str()
                .context("Relaunch result omitted session ID")?;
            self.remember(device, Some(&alias), session_id);
            Ok(value)
        })
        .await
    }
    #[tool(
        description = "Create a fresh Mx-owned simulator matching a template device type and runtime. Copies no source app data or caches; the source can remain booted."
    )]
    async fn mx_create(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<CreateRequest>,
    ) -> CallToolResult {
        cancellable(
            context,
            fleet::create(&r.template, &r.name, r.device_type.as_deref()),
        )
        .await
    }
    #[tool(
        description = "Read the live state of an iOS simulator.",
        annotations(read_only_hint = true)
    )]
    async fn mx_status(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<DeviceRequest>,
    ) -> CallToolResult {
        cancellable(context, async { runtime::device(&request.device).await }).await
    }

    #[tool(
        description = "Inspect the foreground simulator UI as compact semantic elements. Requires AXe. Use returned accessibility identifiers or labels with mx_tap.",
        annotations(read_only_hint = true)
    )]
    async fn mx_ui(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<DeviceRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let expected = self.bound(&request.device)?;
            session::EXPECTED_SESSION
                .scope(expected, runtime::ui(&request.device))
                .await
        })
        .await
    }

    #[tool(
        description = "Tap exactly one UI element by accessibility identifier or label, optionally filtered by role. Ambiguous selectors fail. Inspect UI afterward to verify the result."
    )]
    async fn mx_tap(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<TapRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let expected = self.bound(&request.device)?;
            session::EXPECTED_SESSION
                .scope(expected, runtime::tap(&request.device, request.selector))
                .await
        })
        .await
    }

    #[tool(
        description = "Type text into the focused field of the active app session. Unicode uses the simulator pasteboard and Cmd+V. ASCII uses HID input."
    )]
    async fn mx_type(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<TypeRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let expected = self.bound(&request.device)?;
            session::EXPECTED_SESSION
                .scope(expected, runtime::type_text(&request.device, &request.text))
                .await
        })
        .await
    }

    #[tool(
        description = "Read recent unified logs for an app PID. Returns at most 200 lines / 64 KiB with a truncation flag. Use the mx logs CLI for continuous streaming.",
        annotations(read_only_hint = true)
    )]
    async fn mx_logs(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<LogsRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            runtime::logs(&request.device, request.pid, request.seconds).await
        })
        .await
    }

    #[tool(
        description = "Capture a PNG screenshot when visual inspection is needed. Omit output to save under .mx/screenshots, or provide a new local path. Returns the absolute path and never overwrites a file."
    )]
    async fn mx_screenshot(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<ScreenshotRequest>,
    ) -> CallToolResult {
        use base64::Engine;
        let mut response = cancellable(context, async {
            runtime::screenshot(&request.device, request.output).await
        })
        .await;
        if request.inline && response.is_error != Some(true) {
            let saved_path = response
                .structured_content
                .as_ref()
                .and_then(|value| value.get("path"))
                .and_then(|value| value.as_str());
            match saved_path.and_then(|path| std::fs::read(path).ok()) {
                Some(bytes) if bytes.len() <= 8 * 1024 * 1024 => {
                    response.content.push(ContentBlock::image(
                        base64::engine::general_purpose::STANDARD.encode(bytes),
                        "image/png",
                    ))
                }
                _ => response.content.push(ContentBlock::text(
                    "PNG saved; inline image unavailable or exceeds 8 MiB",
                )),
            }
        }
        response
    }

    #[tool(description = "Terminate the specified application, leaving the simulator booted.")]
    async fn mx_stop(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(request): Parameters<StopRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let expected = self.bound(&request.device)?;
            session::EXPECTED_SESSION
                .scope(expected, runtime::stop(&request.device, &request.bundle_id))
                .await
        })
        .await
    }
    #[tool(
        description = "List persistent app sessions across CLI/server restarts.",
        annotations(read_only_hint = true)
    )]
    async fn mx_sessions(&self) -> CallToolResult {
        result(session::list().map(|sessions| serde_json::json!({"sessions":sessions})))
    }
    #[tool(
        description = "Inspect the bound app and return stable element references and a delta since a revision. First call returns a full screen.",
        annotations(read_only_hint = true)
    )]
    async fn mx_observe(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<ObserveRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let expected = self.bound(&r.device)?;
            session::EXPECTED_SESSION
                .scope(expected, interaction::observe(&r.device, r.since))
                .await
        })
        .await
    }
    #[tool(
        description = "Execute up to 32 actions within one device lock, then wait for stable UI and an optional expected label. Reference taps require their revision."
    )]
    async fn mx_action(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<interaction::ActionRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let expected = self.bound(&r.device)?;
            session::EXPECTED_SESSION
                .scope(expected, interaction::act(r))
                .await
        })
        .await
    }
    #[tool(
        description = "Capture up to 64 named navigation states in one request. Runs semantic actions, verifies expected labels, saves whole-screen PNGs, and writes an agent-readable flow.json manifest containing each UI snapshot and transition result."
    )]
    async fn mx_capture_flow(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<mx::flow::CaptureRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            let expected = self.bound(&r.device)?;
            session::EXPECTED_SESSION
                .scope(expected, mx::flow::capture(r))
                .await
        })
        .await
    }
    #[tool(
        description = "Start continuous app log capture. Returns a stream ID; read new entries using mx_logs_read. Capture lasts until stopped or server exits."
    )]
    async fn mx_logs_start(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<LiveRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            Ok(serde_json::json!({"stream_id":self.logs.start(&r.device,r.pid).await?}))
        })
        .await
    }
    #[tool(
        description = "Read newly captured log entries after a cursor. Reports gaps if the bounded ring buffer overflowed.",
        annotations(read_only_hint = true)
    )]
    async fn mx_logs_read(&self, Parameters(r): Parameters<StreamRequest>) -> CallToolResult {
        result(self.logs.read(&r.stream_id, r.after))
    }
    #[tool(description = "Stop a live log stream and release its subprocess and buffer.")]
    async fn mx_logs_stop(&self, Parameters(r): Parameters<StreamRequest>) -> CallToolResult {
        result(
            self.logs
                .stop(&r.stream_id)
                .map(|_| serde_json::json!({"stopped":true})),
        )
    }
    #[tool(
        description = "Clone a shut-down iOS simulator into an Mx-owned isolated device. Source is never shut down automatically."
    )]
    async fn mx_clone(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<CloneRequest>,
    ) -> CallToolResult {
        cancellable(context, fleet::clone(&r.source, &r.name)).await
    }
    #[tool(
        description = "Boot or reuse an Mx-owned warm simulator, respecting a booted-device capacity limit."
    )]
    async fn mx_boot(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<BootRequest>,
    ) -> CallToolResult {
        cancellable(
            context,
            fleet::boot(&r.device, r.max_booted, r.memory_budget),
        )
        .await
    }
    #[tool(description = "Shut down an Mx-owned simulator after stopping its app session.")]
    async fn mx_shutdown(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<DeviceRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            fleet::shutdown(&r.device).await?;
            Ok(serde_json::json!({"shutdown":true}))
        })
        .await
    }
    #[tool(
        description = "Delete only a shut-down Mx-owned simulator. This permanently removes that clone's data."
    )]
    async fn mx_delete(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<DeviceRequest>,
    ) -> CallToolResult {
        cancellable(context, async {
            fleet::delete(&r.device).await?;
            Ok(serde_json::json!({"deleted":true}))
        })
        .await
    }
    #[tool(
        description = "Measure Mx and simulator process-tree physical footprint and cumulative CPU counters.",
        annotations(read_only_hint = true)
    )]
    async fn mx_metrics(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<DeviceRequest>,
    ) -> CallToolResult {
        cancellable(context, mx::metrics::snapshot(&r.device)).await
    }
    #[tool(
        description = "Return the largest simulator processes by physical footprint so agents can find RAM hotspots without transferring the full process tree.",
        annotations(read_only_hint = true)
    )]
    async fn mx_top(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<TopRequest>,
    ) -> CallToolResult {
        cancellable(context, mx::metrics::top(&r.device, r.limit)).await
    }
    #[tool(
        description = "Report current simulator RAM admission capacity, adaptive budget, disk headroom, and an estimate of additional profiled devices.",
        annotations(read_only_hint = true)
    )]
    async fn mx_capacity(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        cancellable(context, sim::capacity()).await
    }
    #[tool(
        description = "List Mx-owned simulators with boot state, runtime, source, and profile status.",
        annotations(read_only_hint = true)
    )]
    async fn mx_fleet(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        cancellable(context, fleet::inventory()).await
    }
    #[tool(
        description = "Describe the embedded Mx runtime catalog, capability exceptions, retained core services, tested runtime, and native findings.",
        annotations(read_only_hint = true)
    )]
    async fn mx_profile_catalog(&self) -> CallToolResult {
        result(Ok(mx::profile::catalog_summary()))
    }
    #[tool(
        description = "Read profile receipt and disabled-service status for a simulator.",
        annotations(read_only_hint = true)
    )]
    async fn mx_profile_status(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<DeviceRequest>,
    ) -> CallToolResult {
        cancellable(context, mx::profile::status(&r.device)).await
    }
    #[tool(
        description = "Plan, apply, verify, or restore an experimental capability profile on an Mx-owned iOS 26.5 clone. Apply/restore require a stopped app session and reboot the clone. Slim preset disables 172 audited services minus keep categories/capabilities, then exits thirteen optional Watch synchronization agents unless Watch support is retained. Balanced disables 11. Networking and core services are retained."
    )]
    async fn mx_profile(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<mx::profile::Request>,
    ) -> CallToolResult {
        cancellable(context, mx::profile::execute(r)).await
    }
    #[tool(
        description = "Explicitly bind this client to a persistent app session after reconnecting. Refuses mismatched session IDs."
    )]
    async fn mx_use_session(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(r): Parameters<UseSession>,
    ) -> CallToolResult {
        cancellable(context, async {
            let device = runtime::device(&r.device).await?;
            let bound = session::active(&device.udid)?;
            anyhow::ensure!(
                bound.id == r.session_id,
                "Session ID does not match this device"
            );
            self.remember(&device.udid, Some(&r.device), &bound.id);
            Ok(bound)
        })
        .await
    }
}

#[tool_handler(
    name = "mx",
    version = "0.1.0",
    instructions = "Use mx_devices and mx_inspect to resolve the target, then mx_run and semantic UI tools. Prefer mx_capture_flow for several named screens: it performs verified navigation and returns whole-screen PNGs plus an agent-readable flow manifest in one request. Prefer semantic UI over coordinates. UI operations verify the active session PID. Distinct devices can run concurrently. Use mx_capacity, mx_top, mx_profile_status, and mx_fleet to schedule low-memory simulator work."
)]
impl ServerHandler for Server {}

pub async fn serve() -> anyhow::Result<()> {
    let server = Server::default().serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}
