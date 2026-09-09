use crate::{session, ui};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

pub async fn inspect_bound(device: &str, session: &session::Session) -> Result<ui::Screen> {
    let screen = ui::inspect(device).await?;
    session::check_foreground(session, &screen)?;
    Ok(screen)
}
pub async fn observe(device: &str, since: Option<u64>) -> Result<session::Delta> {
    let _lock = session::lock(device)?;
    let mut session = session::active(device)?;
    let screen = inspect_bound(device, &session).await?;
    let delta = session::update(&mut session, screen, since);
    session::save(&session)?;
    Ok(delta)
}

pub async fn observe_expected(
    device: &str,
    expected: Option<&str>,
    timeout_ms: u64,
) -> Result<session::Delta> {
    let _lock = session::lock(device)?;
    let mut session = session::active(device)?;
    let base = session.revision;
    let screen = settle(device, &session, expected, timeout_ms).await?;
    let delta = session::update(&mut session, screen, Some(base));
    session::save(&session)?;
    Ok(delta)
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Tap { selector: ui::Selector },
    TapReference { reference: u64, revision: u64 },
    Type { text: String },
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionRequest {
    pub device: String,
    pub actions: Vec<Action>,
    pub expect_label: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}
fn default_timeout() -> u64 {
    10_000
}

pub async fn act(request: ActionRequest) -> Result<session::Delta> {
    anyhow::ensure!(
        !request.actions.is_empty() && request.actions.len() <= 32,
        "Provide between 1 and 32 actions"
    );
    anyhow::ensure!(
        (100..=60_000).contains(&request.timeout_ms),
        "timeout_ms must be 100..60000"
    );
    let _lock = session::lock(&request.device)?;
    let mut session = session::active(&request.device)?;
    let base = session.revision;
    for action in request.actions {
        let current = settle(&request.device, &session, None, request.timeout_ms).await?;
        match action {
            Action::Tap { selector } => ui::tap_on_screen(&current, selector).await?,
            Action::Type { text } => ui::type_text(&request.device, &text).await?,
            Action::TapReference {
                reference,
                revision,
            } => {
                anyhow::ensure!(
                    revision == session.revision,
                    "Stale UI revision; observe again"
                );
                let previous = session
                    .screen
                    .as_ref()
                    .context("Observe before using references")?
                    .elements
                    .iter()
                    .find(|e| e.reference == Some(reference))
                    .context("Unknown UI reference")?;
                let selector = ui::Selector {
                    identifier: previous.identifier.clone(),
                    label: if previous.identifier.is_none() {
                        previous.label.clone()
                    } else {
                        None
                    },
                    role: Some(previous.role.clone()),
                };
                anyhow::ensure!(
                    current
                        .elements
                        .iter()
                        .filter(|e| selector.matches(e)
                            && e.label == previous.label
                            && e.value == previous.value)
                        .count()
                        == 1,
                    "Referenced element changed; observe again"
                );
                ui::tap_on_screen(&current, selector).await?;
            }
        }
    }
    let screen = settle(
        &request.device,
        &session,
        request.expect_label.as_deref(),
        request.timeout_ms,
    )
    .await?;
    let delta = session::update(&mut session, screen, Some(base));
    session::save(&session)?;
    Ok(delta)
}

pub async fn settle(
    device: &str,
    session: &session::Session,
    expected: Option<&str>,
    timeout_ms: u64,
) -> Result<ui::Screen> {
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let mut previous = None;
        loop {
            let screen = inspect_bound(device, session).await?;
            let matches = expected.is_none_or(|label| {
                screen
                    .elements
                    .iter()
                    .any(|e| e.label.as_deref() == Some(label))
            });
            if matches && previous.as_ref() == Some(&screen) {
                return Ok(screen);
            }
            previous = Some(screen);
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    })
    .await
    .context("UI did not settle with the expected label before timeout")?
}
