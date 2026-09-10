mod mcp;

use anyhow::Result;
use clap::{Parser, Subcommand};
use mx::{fleet, flow, interaction, process, runtime, session, sim, ui};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "An iOS development runtime for AI agents")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check Xcode, CoreSimulator, and AXe using the real local tools.
    Doctor,
    /// Serve agent tools using MCP over stdin/stdout.
    Mcp,
    /// Print MCP client configuration using this executable's absolute path.
    McpConfig,
    /// List persistent app sessions.
    Sessions,
    /// Measure process physical footprint and CPU counters.
    Metrics {
        #[arg(long)]
        device: String,
    },
    /// Apply a profile request from JSON (plan, apply, verify, or restore).
    Profile { file: PathBuf },
    /// Return semantic UI changes since a revision.
    Observe {
        #[arg(long)]
        device: String,
        #[arg(long)]
        since: Option<u64>,
    },
    /// Execute a JSON ActionRequest file and verify the resulting UI.
    Action { file: PathBuf },
    /// Capture named semantic UI states and whole-screen PNGs from a JSON plan.
    CaptureFlow { file: PathBuf },
    /// Install and launch an already-built .app without Xcode work.
    Launch {
        #[arg(long)]
        app: PathBuf,
        #[arg(long)]
        device: String,
        #[arg(long)]
        inspect_ui: bool,
    },
    /// Relaunch the app already installed for an existing Mx session.
    Relaunch {
        #[arg(long)]
        device: String,
        #[arg(long)]
        inspect_ui: bool,
    },
    /// Create a fresh owned simulator with the template's device type and runtime.
    Create {
        #[arg(long)]
        template: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        device_type: Option<String>,
    },
    /// Clone a shut-down simulator into an Mx-owned device.
    Clone {
        #[arg(long)]
        source: String,
        #[arg(long)]
        name: String,
    },
    /// Boot an Mx-owned simulator within a fleet capacity limit.
    Boot {
        #[arg(long)]
        device: String,
        #[arg(long, default_value_t = 8)]
        max_booted: usize,
        #[arg(long, requires = "next_device_mib")]
        memory_budget_mib: Option<u64>,
        #[arg(long, requires = "memory_budget_mib")]
        next_device_mib: Option<u64>,
    },
    /// Shut down an Mx-owned idle simulator.
    Shutdown {
        #[arg(long)]
        device: String,
    },
    /// Delete a shut-down Mx-owned simulator and its data.
    Delete {
        #[arg(long)]
        device: String,
    },
    /// List available iOS simulators as JSON.
    Devices,
    /// Discover the Xcode container and available schemes.
    Inspect {
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Build, install, and launch an iOS application.
    Run {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        scheme: Option<String>,
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        inspect_ui: bool,
    },
    /// Report live simulator state as JSON.
    Status {
        #[arg(long)]
        device: String,
    },
    /// Stream unified logs, or read recent logs with --last <seconds>.
    Logs {
        #[arg(long)]
        device: String,
        #[arg(long)]
        pid: u32,
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=300))]
        last: Option<u32>,
    },
    /// Terminate an app without shutting down its simulator.
    Stop {
        #[arg(long)]
        device: String,
        #[arg(long)]
        bundle_id: String,
    },
    /// Save a simulator screenshot to a new PNG file.
    Screenshot {
        #[arg(long)]
        device: String,
        /// New PNG path. Defaults to .mx/screenshots/<timestamp>-<device>.png.
        output: Option<PathBuf>,
    },
    /// Read compact semantic UI state using AXe.
    Ui {
        #[arg(long)]
        device: String,
    },
    /// Tap a unique accessibility identifier or label.
    Tap {
        #[arg(long)]
        device: String,
        #[arg(long, required_unless_present = "label", conflicts_with = "label")]
        id: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        role: Option<String>,
    },
    /// Type text into the active app, using the simulator pasteboard for Unicode.
    Type {
        #[arg(long)]
        device: String,
        #[arg(allow_hyphen_values = true)]
        text: String,
    },
}

fn print(value: impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Doctor => {
            let xcode = process::output("xcodebuild", &["-version".into()]).await?;
            let simulators = sim::list().await?;
            let axe_path = ui::bridge_path();
            let axe = process::output(&axe_path, &["--version".into()]).await?;
            print(serde_json::json!({
                "ready": true,
                "xcode": xcode.trim(),
                "axe": axe.trim(),
                "axe_path": axe_path,
                "simulators": simulators,
                "state_dir": session::root()?
            }))?;
        }
        Commands::Launch {
            app,
            device,
            inspect_ui,
        } => print(
            runtime::launch(runtime::LaunchRequest {
                app,
                device,
                inspect_ui,
            })
            .await?,
        )?,
        Commands::Relaunch { device, inspect_ui } => {
            print(runtime::relaunch(runtime::RelaunchRequest { device, inspect_ui }).await?)?
        }
        Commands::Create {
            template,
            name,
            device_type,
        } => print(fleet::create(&template, &name, device_type.as_deref()).await?)?,
        Commands::Mcp => mcp::serve().await?,
        Commands::McpConfig => {
            let mut server = serde_json::json!({
                "command": std::env::current_exe()?,
                "args": ["mcp"]
            });
            let mut env = serde_json::Map::new();
            for key in ["MX_AXE_PATH", "MX_STATE_DIR"] {
                if let Ok(value) = std::env::var(key) {
                    env.insert(key.into(), value.into());
                }
            }
            if !env.is_empty() {
                server["env"] = env.into();
            }
            print(serde_json::json!({"mcpServers": {"mx": server}}))?;
        }
        Commands::Sessions => print(session::list()?)?,
        Commands::Metrics { device } => print(mx::metrics::snapshot(&device).await?)?,
        Commands::Profile { file } => {
            print(mx::profile::execute(serde_json::from_slice(&std::fs::read(file)?)?).await?)?
        }
        Commands::Observe { device, since } => print(interaction::observe(&device, since).await?)?,
        Commands::Action { file } => {
            print(interaction::act(serde_json::from_slice(&std::fs::read(file)?)?).await?)?
        }
        Commands::CaptureFlow { file } => {
            print(flow::capture(serde_json::from_slice(&std::fs::read(file)?)?).await?)?
        }
        Commands::Clone { source, name } => print(fleet::clone(&source, &name).await?)?,
        Commands::Boot {
            device,
            max_booted,
            memory_budget_mib,
            next_device_mib,
        } => {
            let budget =
                memory_budget_mib
                    .zip(next_device_mib)
                    .map(|(limit_mib, next_device_mib)| sim::MemoryBudget {
                        limit_mib,
                        next_device_mib,
                    });
            print(fleet::boot(&device, max_booted, budget).await?)?;
        }
        Commands::Shutdown { device } => {
            fleet::shutdown(&device).await?;
            print(serde_json::json!({"shutdown":true}))?;
        }
        Commands::Delete { device } => {
            fleet::delete(&device).await?;
            print(serde_json::json!({"deleted":true}))?;
        }
        Commands::Devices => print(sim::list().await?)?,
        Commands::Inspect { project } => print(runtime::inspect(project).await?)?,
        Commands::Run {
            project,
            scheme,
            device,
            inspect_ui,
        } => print(
            runtime::run(runtime::RunRequest {
                project,
                scheme,
                device,
                inspect_ui,
            })
            .await?,
        )?,
        Commands::Status { device } => print(runtime::device(&device).await?)?,
        Commands::Stop { device, bundle_id } => print(runtime::stop(&device, &bundle_id).await?)?,
        Commands::Screenshot { device, output } => {
            print(runtime::screenshot(&device, output).await?)?
        }
        Commands::Ui { device } => print(runtime::ui(&device).await?)?,
        Commands::Tap {
            device,
            id,
            label,
            role,
        } => print(
            runtime::tap(
                &device,
                ui::Selector {
                    identifier: id,
                    label,
                    role,
                },
            )
            .await?,
        )?,
        Commands::Type { device, text } => print(runtime::type_text(&device, &text).await?)?,
        Commands::Logs { device, pid, last } => match last {
            Some(seconds) => print(runtime::logs(&device, pid, seconds).await?)?,
            None => {
                let device = runtime::booted_device(&device).await?;
                process::stream(
                    "xcrun",
                    &process::strings(&[
                        "simctl",
                        "spawn",
                        &device.udid,
                        "log",
                        "stream",
                        "--style",
                        "ndjson",
                        "--level",
                        "debug",
                        "--predicate",
                        &format!("processIdentifier == {pid}"),
                    ]),
                )
                .await?;
            }
        },
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();
    let result = tokio::select! {
        result = execute(cli) => Some(result),
        _ = tokio::signal::ctrl_c() => None,
    };
    if result.is_none() {
        std::process::exit(130);
    }
    if let Some(Err(error)) = result {
        eprintln!("mx: {error:#}");
        std::process::exit(1);
    }
}
