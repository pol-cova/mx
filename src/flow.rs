use crate::{interaction, runtime, session, ui};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureState {
    pub name: String,
    pub from: Option<String>,
    pub transition: Option<String>,
    #[serde(default)]
    pub actions: Vec<interaction::Action>,
    pub expect_label: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureRequest {
    pub device: String,
    pub output_dir: PathBuf,
    pub states: Vec<CaptureState>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    10_000
}

fn validate_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name.len() <= 80
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
        "Flow state names must contain only letters, numbers, '-' or '_'"
    );
    Ok(())
}

fn write_viewer(path: &Path, states: &Value) -> Result<()> {
    let data = serde_json::to_string(states)?.replace('<', "\\u003c");
    std::fs::write(
        path,
        include_str!("flow-viewer.html").replace("__MX_FLOW_DATA__", &data),
    )?;
    Ok(())
}

async fn run_actions(
    device: &str,
    bound: &session::Session,
    current: &ui::Screen,
    actions: Vec<interaction::Action>,
) -> Result<()> {
    let _ = (device, bound);
    for action in actions {
        match action {
            interaction::Action::Tap { selector } => ui::tap_on_screen(current, selector).await?,
            interaction::Action::Type { text } => ui::type_text(device, &text).await?,
            interaction::Action::TapReference {
                reference,
                revision,
            } => {
                anyhow::ensure!(
                    revision == bound.revision,
                    "Stale UI revision; observe again"
                );
                let previous = bound
                    .screen
                    .as_ref()
                    .context("Observe before using references")?
                    .elements
                    .iter()
                    .find(|element| element.reference == Some(reference))
                    .context("Unknown UI reference")?;
                let selector = ui::Selector {
                    identifier: previous.identifier.clone(),
                    label: previous
                        .identifier
                        .is_none()
                        .then(|| previous.label.clone())
                        .flatten(),
                    role: Some(previous.role.clone()),
                };
                ui::tap_on_screen(current, selector).await?;
            }
        }
    }
    Ok(())
}

pub async fn capture(request: CaptureRequest) -> Result<Value> {
    anyhow::ensure!(
        !request.states.is_empty() && request.states.len() <= 64,
        "Provide between 1 and 64 flow states"
    );
    anyhow::ensure!(
        (100..=60_000).contains(&request.timeout_ms),
        "timeout_ms must be 100..60000"
    );
    let manifest_path = request.output_dir.join("flow.json");
    let viewer_path = request.output_dir.join("flow.html");
    anyhow::ensure!(
        manifest_path.symlink_metadata().is_err() && viewer_path.symlink_metadata().is_err(),
        "Flow output already exists in {}",
        request.output_dir.display()
    );
    std::fs::create_dir_all(&request.output_dir)?;
    let device_id = match session::for_device(&request.device).await {
        Ok(bound) => bound.device,
        Err(_) => runtime::booted_device(&request.device).await?.udid,
    };
    let flow_started = std::time::Instant::now();
    let _lock = session::lock(&device_id).await?;
    let mut bound = session::active(&device_id).await?;
    let mut current = interaction::inspect_bound(&device_id, &bound).await?;
    let mut names = HashSet::new();
    let mut captured = Vec::with_capacity(request.states.len());
    let mut previous_name: Option<String> = None;

    for (index, state) in request.states.into_iter().enumerate() {
        validate_name(&state.name)?;
        if let Some(parent) = state.from.as_ref() {
            validate_name(parent)?;
            anyhow::ensure!(
                names.contains(parent),
                "Flow state parent must appear before its child"
            );
        }
        anyhow::ensure!(
            names.insert(state.name.clone()),
            "Duplicate flow state name"
        );
        let screenshot_file = format!("{:02}-{}.png", index + 1, state.name);
        let screenshot_path = request.output_dir.join(&screenshot_file);
        anyhow::ensure!(
            screenshot_path.symlink_metadata().is_err(),
            "Flow screenshot already exists: {}",
            screenshot_path.display()
        );

        let transition_started = std::time::Instant::now();
        let base_revision = bound.revision;
        let has_actions = !state.actions.is_empty();
        run_actions(&device_id, &bound, &current, state.actions).await?;
        let current_matches = state.expect_label.as_ref().is_some_and(|expected| {
            current
                .elements
                .iter()
                .any(|element| element.label.as_ref() == Some(expected))
        });
        if has_actions || !current_matches {
            current = interaction::settle(
                &device_id,
                &bound,
                state.expect_label.as_deref(),
                request.timeout_ms,
            )
            .await?;
        }
        let delta = session::update(&mut bound, current.clone(), Some(base_revision));
        session::save(&bound).await?;
        let transition_ms = transition_started.elapsed().as_millis();
        let screen = current.clone();
        let screenshot_started = std::time::Instant::now();
        let image = runtime::screenshot_device(&device_id, screenshot_path).await?;
        let screenshot_ms = screenshot_started.elapsed().as_millis();
        let from = state.from.or_else(|| previous_name.clone());
        captured.push(json!({"index":index + 1,"name":state.name,"from":from,"transition":state.transition,"expected_label":state.expect_label,"revision":delta.revision,"screen":screen,"screenshot":image["path"],"screenshot_file":screenshot_file,"transition_ms":transition_ms,"screenshot_ms":screenshot_ms,"total_ms":transition_ms + screenshot_ms}));
        previous_name = Some(state.name);
    }

    let elapsed_ms = flow_started.elapsed().as_millis();
    let manifest =
        json!({"version":1,"device":device_id,"elapsed_ms":elapsed_ms,"states":captured});
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    write_viewer(&viewer_path, &manifest["states"])?;
    Ok(
        json!({"manifest":manifest_path.canonicalize()?,"viewer":viewer_path.canonicalize()?,"elapsed_ms":elapsed_ms,"state_count":manifest["states"].as_array().map_or(0, Vec::len),"states":manifest["states"]}),
    )
}
