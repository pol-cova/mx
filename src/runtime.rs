use crate::{
    diagnostics, interaction,
    process::{self, output, strings},
    profile, session, sim, ui, xcode,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Instant};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    /// Xcode project, workspace, or containing directory. Defaults to the server's working directory.
    #[serde(default = "default_project")]
    pub project: PathBuf,
    pub scheme: Option<String>,
    /// Simulator UDID or unambiguous name. Omit only if one device can be selected.
    pub device: Option<String>,
    #[serde(default)]
    pub inspect_ui: bool,
}
fn default_project() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub project: PathBuf,
    pub scheme: String,
    pub device: String,
    pub bundle_id: String,
    pub pid: u32,
    pub app: PathBuf,
    pub elapsed_ms: u128,
    pub build_log: PathBuf,
    pub session_id: String,
    pub timings_ms: std::collections::BTreeMap<String, u128>,
    pub diagnostics: Vec<diagnostics::Diagnostic>,
    pub diagnostic_delta: diagnostics::Delta,
    pub ui: Option<ui::Screen>,
    pub ui_error: Option<String>,
}

pub async fn inspect(project: PathBuf) -> Result<Value> {
    let project = xcode::discover(&project)?;
    let schemes = xcode::schemes(&project).await?;
    Ok(json!({"project": project, "schemes": schemes}))
}

pub async fn run(request: RunRequest) -> Result<RunResult> {
    let started = Instant::now();
    let project = xcode::discover(&request.project)?;
    let (scheme, device) = if let Some(scheme) = request
        .scheme
        .as_ref()
        .map(|scheme| scheme.trim())
        .filter(|scheme| !scheme.is_empty())
    {
        let device = sim::select(&sim::list().await?, request.device.as_deref())?;
        (scheme.to_owned(), device)
    } else {
        anyhow::ensure!(
            request
                .scheme
                .as_deref()
                .is_none_or(|scheme| !scheme.trim().is_empty()),
            "Scheme cannot be empty"
        );
        let (schemes, devices) = tokio::join!(xcode::schemes(&project), sim::list());
        let device = sim::select(&devices?, request.device.as_deref())?;
        (xcode::select_scheme(&schemes?, None)?, device)
    };
    let _device_lock = session::lock(&device.udid).await?;
    let mut timings_ms = std::collections::BTreeMap::new();
    timings_ms.insert("discovery".into(), started.elapsed().as_millis());
    let artifacts = project
        .parent()
        .context("Project has no parent")?
        .join(".mx")
        .join(&device.udid);
    std::fs::create_dir_all(&artifacts)?;
    let run_lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(artifacts.join("run.lock"))?;
    run_lock
        .try_lock()
        .context("Another Mx run is using this project; wait for it to finish")?;

    let mut args = xcode::project_args(&project);
    args.extend(strings(&[
        "-scheme",
        &scheme,
        "-configuration",
        "Debug",
        "-destination",
        &format!("platform=iOS Simulator,id={}", device.udid),
        "-derivedDataPath",
        &artifacts.join("DerivedData").to_string_lossy(),
        "CODE_SIGNING_ALLOWED=NO",
    ]));
    eprintln!("Resolving application target...");
    let mut settings_args = args.clone();
    settings_args.extend(strings(&["-showBuildSettings", "-json"]));
    let settings_started = Instant::now();
    let settings = tokio::spawn(async move {
        let raw = output("xcodebuild", &settings_args).await?;
        anyhow::Ok(xcode::app_from_settings(&raw)?)
    });
    eprintln!("Building {scheme}...");
    args.push("build".into());
    let log_path = artifacts.join("build.log");
    let phase = Instant::now();
    let build = process::build(&args, &log_path).await;
    let before: Vec<diagnostics::Diagnostic> = std::fs::read(artifacts.join("diagnostics.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let diagnostics = diagnostics::read(&log_path).unwrap_or_default();
    let diagnostic_delta = diagnostics::diff(&before, &diagnostics);
    std::fs::write(
        artifacts.join("diagnostics.json"),
        serde_json::to_vec_pretty(&diagnostics)?,
    )?;
    if let Err(error) = build {
        settings.abort();
        return Err(diagnostics::BuildFailure {
            build_log: log_path,
            diagnostics,
            reason: error.to_string(),
            delta: diagnostic_delta,
        }
        .into());
    }
    let app = settings
        .await
        .context("Settings task panicked")?
        .context("Build settings failed")?;
    timings_ms.insert("settings".into(), settings_started.elapsed().as_millis());
    timings_ms.insert("build".into(), phase.elapsed().as_millis());
    eprintln!("Preparing {}...", device.name);
    let (bundle_id, pid, launch_timings) = install_launch(&device, &app).await?;
    timings_ms.extend(launch_timings);
    let mut bound = session::bind(
        device.udid.clone(),
        project.clone(),
        scheme.clone(),
        bundle_id.clone(),
        pid,
    )
    .await?;
    let (ui, ui_error) = if request.inspect_ui {
        match interaction::settle(&device.udid, &bound, None, 10_000).await {
            Ok(screen) => {
                session::update(&mut bound, screen.clone(), None);
                session::save(&bound).await?;
                (Some(screen), None)
            }
            Err(error) => (None, Some(error.to_string())),
        }
    } else {
        (None, None)
    };
    Ok(RunResult {
        project,
        scheme,
        device: device.udid,
        bundle_id,
        pid,
        app,
        elapsed_ms: started.elapsed().as_millis(),
        build_log: log_path,
        session_id: bound.id,
        timings_ms,
        diagnostics,
        diagnostic_delta,
        ui,
        ui_error,
    })
}

async fn install_launch(
    device: &sim::Device,
    app: &std::path::Path,
) -> Result<(String, u32, std::collections::BTreeMap<String, u128>)> {
    let mut timings_ms = std::collections::BTreeMap::new();
    anyhow::ensure!(
        app.is_dir(),
        "Built application missing at {}",
        app.display()
    );
    let bundle_id = output(
        "/usr/libexec/PlistBuddy",
        &strings(&[
            "-c",
            "Print :CFBundleIdentifier",
            &app.join("Info.plist").to_string_lossy(),
        ]),
    )
    .await?
    .trim()
    .to_owned();
    anyhow::ensure!(
        !bundle_id.is_empty(),
        "Application bundle identifier is empty"
    );
    let phase = Instant::now();
    let fresh_boot = device.state != "Booted";
    sim::boot(device).await?;
    if fresh_boot {
        profile::trim_after_boot(&device.udid).await?;
    }
    timings_ms.insert("boot".into(), phase.elapsed().as_millis());
    let phase = Instant::now();
    sim::call(&["install", &device.udid, &app.to_string_lossy()]).await?;
    timings_ms.insert("install".into(), phase.elapsed().as_millis());
    let phase = Instant::now();
    let pid = terminate_and_launch(&device.udid, &bundle_id).await?;
    timings_ms.insert("launch".into(), phase.elapsed().as_millis());
    Ok((bundle_id, pid, timings_ms))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    /// A previously built simulator .app. Build once, reuse across compatible simulators.
    pub app: PathBuf,
    pub device: String,
    #[serde(default)]
    pub inspect_ui: bool,
}
pub async fn launch(request: LaunchRequest) -> Result<Value> {
    let started = Instant::now();
    let app = request
        .app
        .canonicalize()
        .context("Prebuilt app does not exist")?;
    let device = device(&request.device).await?;
    let _lock = session::lock(&device.udid).await?;
    let (bundle_id, pid, timings) = install_launch(&device, &app).await?;
    let mut bound = session::bind(
        device.udid.clone(),
        app.clone(),
        "prebuilt".into(),
        bundle_id.clone(),
        pid,
    )
    .await?;
    let (screen, ui_error) = if request.inspect_ui {
        match interaction::settle(&device.udid, &bound, None, 10_000).await {
            Ok(screen) => {
                session::update(&mut bound, screen.clone(), None);
                session::save(&bound).await?;
                (Some(screen), None)
            }
            Err(error) => (None, Some(error.to_string())),
        }
    } else {
        (None, None)
    };
    Ok(
        json!({"session_id":bound.id,"device":device.udid,"bundle_id":bundle_id,"pid":pid,"app":app,"ui":screen,"ui_error":ui_error,"timings_ms":timings,"elapsed_ms":started.elapsed().as_millis(),"built":false}),
    )
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelaunchRequest {
    /// Simulator UDID with an existing Mx session and installed app.
    pub device: String,
    #[serde(default)]
    pub inspect_ui: bool,
}

pub async fn relaunch(request: RelaunchRequest) -> Result<Value> {
    let started = Instant::now();
    let previous = session::for_device(&request.device).await?;
    let _lock = session::lock(&previous.device).await?;
    let phase = Instant::now();
    let pid = terminate_and_launch(&previous.device, &previous.bundle_id).await?;
    let launch_ms = phase.elapsed().as_millis();
    let mut bound = session::bind(
        previous.device.clone(),
        previous.project,
        previous.scheme,
        previous.bundle_id.clone(),
        pid,
    )
    .await?;
    let (screen, ui_error) = if request.inspect_ui {
        match interaction::settle(&previous.device, &bound, None, 10_000).await {
            Ok(screen) => {
                session::update(&mut bound, screen.clone(), None);
                session::save(&bound).await?;
                (Some(screen), None)
            }
            Err(error) => (None, Some(error.to_string())),
        }
    } else {
        (None, None)
    };
    Ok(
        json!({"session_id":bound.id,"device":previous.device,"bundle_id":previous.bundle_id,"pid":pid,"ui":screen,"ui_error":ui_error,"timings_ms":{"launch":launch_ms},"elapsed_ms":started.elapsed().as_millis(),"installed":false}),
    )
}

pub async fn device(requested: &str) -> Result<sim::Device> {
    sim::select(&sim::list().await?, Some(requested))
}

pub async fn booted_device(requested: &str) -> Result<sim::Device> {
    let device = device(requested).await?;
    anyhow::ensure!(
        device.state == "Booted",
        "Simulator {} is {}; run the app first",
        device.name,
        device.state
    );
    Ok(device)
}

pub async fn stop(requested: &str, bundle_id: &str) -> Result<Value> {
    let device = booted_device(requested).await?;
    let _lock = session::lock(&device.udid).await?;
    if session::EXPECTED_SESSION.try_with(|_| ()).is_ok() {
        let bound = session::active(&device.udid).await?;
        anyhow::ensure!(
            bound.bundle_id == bundle_id,
            "Bundle ID does not belong to this session"
        );
    }
    let terminated = match sim::call(&["terminate", &device.udid, bundle_id]).await {
        Ok(_) => true,
        Err(error) if app_is_already_stopped(&error) => false,
        Err(error) => return Err(error),
    };
    if let Ok(mut session) = session::for_device(&device.udid).await
        && session.bundle_id == bundle_id
    {
        session.active = false;
        session::save(&session).await?;
    }
    Ok(json!({
        "device": device.udid,
        "bundle_id": bundle_id,
        "terminated": terminated,
        "already_stopped": !terminated
    }))
}

async fn terminate_and_launch(udid: &str, bundle_id: &str) -> Result<u32> {
    if std::env::var_os("MX_TEST_ROOT").is_none() && crate::native::coresim::launch_supported() {
        let udid = udid.to_owned();
        let bundle_id = bundle_id.to_owned();
        return tokio::task::spawn_blocking(move || {
            crate::native::coresim::terminate_and_launch(&udid, &bundle_id)
        })
        .await?;
    }
    let launched = sim::call(&["launch", "--terminate-running-process", udid, bundle_id]).await?;
    launched
        .trim()
        .rsplit_once(':')
        .context("Unexpected simctl launch response")?
        .1
        .trim()
        .parse()
        .context("Unexpected application PID")
}

fn app_is_already_stopped(error: &anyhow::Error) -> bool {
    error.to_string().contains("found nothing to terminate")
}

fn default_screenshot_path(device: &str) -> PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let device = device
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    PathBuf::from(".mx/screenshots").join(format!("{timestamp}-{device}.png"))
}

pub async fn screenshot(requested: &str, output: Option<PathBuf>) -> Result<Value> {
    let device = booted_device(requested).await?;
    let path = output.unwrap_or_else(|| default_screenshot_path(&device.name));
    screenshot_device(&device.udid, path).await
}

pub async fn screenshot_device(device: &str, path: PathBuf) -> Result<Value> {
    anyhow::ensure!(
        path.symlink_metadata().is_err(),
        "Screenshot output already exists: {}",
        path.display()
    );
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = tempfile::tempdir_in(parent)?;
    let capture = temporary.path().join("capture.png");
    sim::call(&[
        "io",
        device,
        "screenshot",
        "--type=png",
        &capture.to_string_lossy(),
    ])
    .await?;
    std::fs::hard_link(&capture, &path)
        .with_context(|| format!("Cannot save screenshot at {}", path.display()))?;
    Ok(json!({"device": device, "path": path.canonicalize()?, "mime_type": "image/png"}))
}

pub async fn ui(requested: &str) -> Result<ui::Screen> {
    let device = booted_device(requested).await?;
    let _lock = session::lock(&device.udid).await?;
    let mut bound = session::active(&device.udid).await?;
    let screen = interaction::inspect_bound(&device.udid, &bound).await?;
    session::update(&mut bound, screen, None);
    session::save(&bound).await?;
    bound
        .screen
        .map(Arc::unwrap_or_clone)
        .context("UI snapshot was not saved")
}

pub async fn tap(requested: &str, selector: ui::Selector) -> Result<Value> {
    let device = booted_device(requested).await?;
    let _lock = session::lock(&device.udid).await?;
    let bound = session::active(&device.udid).await?;
    let screen = match cached_tap_screen(&bound, &selector) {
        Some(screen) => screen,
        None => {
            let inspected = interaction::inspect_bound(&device.udid, &bound).await?;
            let mut bound = bound;
            session::update(&mut bound, inspected, None);
            session::save(&bound).await?;
            Arc::unwrap_or_clone(bound.screen.expect("UI snapshot was not saved"))
        }
    };
    ui::tap_on_screen(&screen, selector).await?;
    Ok(json!({"device": device.udid, "tapped": true}))
}

fn cached_tap_screen(session: &session::Session, selector: &ui::Selector) -> Option<ui::Screen> {
    let screen = session.screen.as_ref()?;
    session::check_foreground(session, screen).ok()?;
    let matched: Vec<_> = screen
        .elements
        .iter()
        .filter(|element| selector.matches(element))
        .collect();
    (matched.len() == 1 && (matched[0].frame.is_some() || crate::native::using_test_transport()))
        .then(|| Arc::unwrap_or_clone(screen.clone()))
}

pub async fn type_text(requested: &str, text: &str) -> Result<Value> {
    let device = booted_device(requested).await?;
    let _lock = session::lock(&device.udid).await?;
    let bound = session::active(&device.udid).await?;
    let needs_inspect = bound
        .screen
        .as_ref()
        .is_none_or(|screen| session::check_foreground(&bound, screen).is_err());
    if needs_inspect {
        interaction::inspect_bound(&device.udid, &bound).await?;
    }
    ui::type_text(&device.udid, text).await?;
    Ok(json!({"device": device.udid, "typed_characters": text.chars().count()}))
}

pub async fn logs(requested: &str, pid: u32, seconds: u32) -> Result<Value> {
    anyhow::ensure!(
        (1..=300).contains(&seconds),
        "seconds must be between 1 and 300"
    );
    let device = booted_device(requested).await?;
    let logs = sim::call(&[
        "spawn",
        &device.udid,
        "log",
        "show",
        "--style",
        "ndjson",
        "--debug",
        "--info",
        "--last",
        &format!("{seconds}s"),
        "--predicate",
        &format!("processIdentifier == {pid}"),
    ])
    .await?;
    // Bound tool responses even if a noisy application emits thousands of events.
    let lines: Vec<_> = logs.lines().collect();
    let mut bytes = 0;
    let mut selected = Vec::new();
    for line in lines.iter().rev().take(200) {
        if bytes + line.len() > 64 * 1024 {
            break;
        }
        bytes += line.len();
        selected.push(*line);
    }
    selected.reverse();
    Ok(
        json!({"device": device.udid, "pid": pid, "seconds": seconds,
        "truncated": selected.len() < lines.len(), "logs": selected.join("\n")}),
    )
}

#[cfg(test)]
mod tests {
    use super::app_is_already_stopped;

    #[test]
    fn recognizes_simctl_already_stopped_response() {
        let absent = anyhow::anyhow!("Simulator failed: found nothing to terminate");
        let other = anyhow::anyhow!("Simulator failed: permission denied");
        assert!(app_is_already_stopped(&absent));
        assert!(!app_is_already_stopped(&other));
    }
}
