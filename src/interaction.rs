use crate::{session, ui};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::{Duration, Instant};

const FRESH_WINDOW_MS: u128 = 300;

pub struct PollBackoff {
    next_ms: u64,
    cap_ms: u64,
}
impl PollBackoff {
    pub fn new(start_ms: u64, cap_ms: u64) -> Self {
        Self {
            next_ms: start_ms,
            cap_ms,
        }
    }
    pub async fn wait(&mut self) {
        tokio::time::sleep(Duration::from_millis(self.next_ms.min(self.cap_ms))).await;
        self.next_ms = self.next_ms * 3 / 2;
    }
}

pub async fn inspect_bound(device: &str, session: &session::Session) -> Result<ui::Screen> {
    let screen = crate::native::inspect_pid(device, Some(session.pid)).await?;
    session::check_foreground(session, &screen)?;
    Ok(screen)
}
pub async fn observe(device: &str, since: Option<u64>) -> Result<session::Delta> {
    let _lock = session::lock(device).await?;
    let mut session = session::active(device).await?;
    let screen = inspect_bound(device, &session).await?;
    let delta = session::update(&mut session, screen, since);
    session::save(&session).await?;
    Ok(delta)
}

pub async fn observe_expected(
    device: &str,
    expected: Option<&str>,
    timeout_ms: u64,
) -> Result<session::Delta> {
    let _lock = session::lock(device).await?;
    let mut session = session::active(device).await?;
    let base = session.revision;
    let screen = settle(device, &session, expected, timeout_ms).await?;
    let delta = session::update(&mut session, screen, Some(base));
    session::save(&session).await?;
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

type Cache = Option<(Instant, ui::Screen)>;

pub async fn act(request: ActionRequest) -> Result<session::Delta> {
    anyhow::ensure!(
        !request.actions.is_empty() && request.actions.len() <= 32,
        "Provide between 1 and 32 actions"
    );
    anyhow::ensure!(
        (100..=60_000).contains(&request.timeout_ms),
        "timeout_ms must be 100..60000"
    );
    let _lock = session::lock(&request.device).await?;
    let mut session = session::active(&request.device).await?;
    let base = session.revision;
    let mut cache = None;
    for action in request.actions {
        let (current, _) = settle_with_cache(
            &request.device,
            &session,
            None,
            request.timeout_ms,
            &mut cache,
        )
        .await?;
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
    let (screen, _) = settle_with_cache(
        &request.device,
        &session,
        request.expect_label.as_deref(),
        request.timeout_ms,
        &mut cache,
    )
    .await?;
    let delta = session::update(&mut session, screen, Some(base));
    session::save(&session).await?;
    Ok(delta)
}

const SETTLE_POLL: Duration = Duration::from_millis(50);
const SETTLE_POLL_WITHOUT_HASH: Duration = Duration::from_millis(150);

fn has_expected_label(screen: &ui::Screen, expected: Option<&str>) -> bool {
    expected.is_none_or(|label| {
        screen
            .elements
            .iter()
            .any(|element| element.label.as_deref() == Some(label))
    })
}

fn screen_hash(screen: &ui::Screen) -> Option<&str> {
    screen.hash.as_deref().filter(|hash| !hash.is_empty())
}

fn hashes_equal(left: Option<&str>, right: Option<&str>) -> bool {
    matches!((left, right), (Some(left), Some(right)) if left == right)
}

/// Pause before a stability sample. Replace with IOSurface `wait_quiet` later.
async fn wait_stability_window() {
    tokio::time::sleep(SETTLE_POLL).await;
}

pub async fn settle(
    device: &str,
    session: &session::Session,
    expected: Option<&str>,
    timeout_ms: u64,
) -> Result<ui::Screen> {
    let mut cache = None;
    settle_with_cache(device, session, expected, timeout_ms, &mut cache)
        .await
        .map(|screen| screen.0)
}

async fn settle_with_cache(
    device: &str,
    session: &session::Session,
    expected: Option<&str>,
    timeout_ms: u64,
    cache: &mut Cache,
) -> Result<(ui::Screen, Instant)> {
    let matches_label = |screen: &ui::Screen| {
        expected.is_none_or(|label| {
            screen
                .elements
                .iter()
                .any(|e| e.label.as_deref() == Some(label))
        })
    };
    if let Some((at, cached)) = cache.as_ref()
        && at.elapsed().as_millis() < FRESH_WINDOW_MS
    {
        let screen = inspect_bound(device, session).await?;
        if screen == *cached && matches_label(&screen) {
            let now = Instant::now();
            *cache = Some((now, screen.clone()));
            return Ok((screen, now));
        }
        *cache = None;
    }
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let mut screen = inspect_bound(device, session).await?;
        loop {
            if !has_expected_label(&screen, expected) {
                tokio::time::sleep(SETTLE_POLL).await;
                match crate::native::inspect_hash(device, Some(session.pid)).await? {
                    Some(hash) if hashes_equal(screen_hash(&screen), Some(hash.as_str())) => {
                        continue;
                    }
                    Some(_) => {
                        screen = inspect_bound(device, session).await?;
                    }
                    None => {
                        tokio::time::sleep(SETTLE_POLL_WITHOUT_HASH.saturating_sub(SETTLE_POLL))
                            .await;
                        screen = inspect_bound(device, session).await?;
                    }
                }
                continue;
            }

            wait_stability_window().await;
            match crate::native::inspect_hash(device, Some(session.pid)).await? {
                Some(hash) if hashes_equal(screen_hash(&screen), Some(hash.as_str())) => {
                    let now = Instant::now();
                    *cache = Some((now, screen.clone()));
                    return Ok((screen, now));
                }
                Some(_) => {
                    screen = inspect_bound(device, session).await?;
                }
                None => {
                    tokio::time::sleep(SETTLE_POLL_WITHOUT_HASH.saturating_sub(SETTLE_POLL)).await;
                    let next = inspect_bound(device, session).await?;
                    if has_expected_label(&next, expected)
                        && (hashes_equal(screen_hash(&screen), screen_hash(&next))
                            || screen_hash(&next).is_none())
                    {
                        let now = Instant::now();
                        *cache = Some((now, next.clone()));
                        return Ok((next, now));
                    }
                    screen = next;
                }
            }
        }
    })
    .await
    .context("UI did not settle with the expected label before timeout")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn poll_backoff_starts_fast_and_caps() {
        let mut waits = Vec::new();
        let mut backoff = PollBackoff::new(50, 300);
        for _ in 0..14 {
            waits.push(backoff.next_ms.min(backoff.cap_ms));
            backoff.next_ms = backoff.next_ms * 3 / 2;
        }
        assert_eq!(&waits[..6], &[50, 75, 112, 168, 252, 300]);
        assert!(waits[6..].iter().all(|ms| *ms == 300));
    }
}
