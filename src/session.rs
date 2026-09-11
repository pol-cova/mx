use crate::ui::{Element, Screen};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::PathBuf,
    sync::Arc,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub device: String,
    pub project: PathBuf,
    pub scheme: String,
    pub bundle_id: String,
    pub pid: u32,
    pub active: bool,
    pub revision: u64,
    pub next_reference: u64,
    pub screen: Option<Arc<Screen>>,
}

pub(crate) async fn blocking<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .context("Session filesystem task panicked")?
}

fn root_blocking() -> Result<PathBuf> {
    let root = if let Some(path) = std::env::var_os("MX_STATE_DIR") {
        PathBuf::from(path)
    } else {
        PathBuf::from(std::env::var_os("HOME").context("HOME or MX_STATE_DIR is required")?)
            .join("Library/Application Support/Mx")
    };
    std::fs::create_dir_all(&root)?;
    Ok(root)
}
pub fn root() -> Result<PathBuf> {
    root_blocking()
}
fn key(value: &str) -> Result<&str> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "Invalid session/device identifier"
    );
    Ok(value)
}
fn lock_file(device: &str) -> Result<File> {
    let path = root_blocking()?.join(format!("{}.lock", key(device)?));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock()
        .context("Device is busy in another Mx operation; retry after it finishes")?;
    Ok(file)
}
pub async fn lock(device: &str) -> Result<File> {
    let device = device.to_owned();
    blocking(move || lock_file(&device)).await
}
fn write_session(session: &Session) -> Result<()> {
    use std::io::Write;
    let root = root_blocking()?;
    let mut temp = tempfile::NamedTempFile::new_in(&root)?;
    serde_json::to_writer(&mut temp, session)?;
    temp.flush()?;
    temp.persist(root.join(format!("{}.session.json", key(&session.device)?)))?;
    Ok(())
}
pub async fn save(session: &Session) -> Result<()> {
    let session = session.clone();
    blocking(move || write_session(&session)).await
}
pub(crate) fn read(device: &str) -> Result<Session> {
    let bytes = std::fs::read(root_blocking()?.join(format!("{}.session.json", key(device)?)))
        .context("No Mx session for this device; use mx run first")?;
    let session: Session = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(session.device == device, "Session/device mismatch");
    Ok(session)
}
pub async fn for_device(device: &str) -> Result<Session> {
    let device = device.to_owned();
    blocking(move || read(&device)).await
}
tokio::task_local! {
    // Rechecked after the device lock is acquired, so discovery cannot race session replacement.
    pub static EXPECTED_SESSION: String;
}
pub fn check_expected(session: &Session) -> Result<()> {
    if let Ok(expected) = EXPECTED_SESSION.try_with(Clone::clone) {
        anyhow::ensure!(
            session.id == expected,
            "Another client replaced this app session; refusing to target its app"
        );
    }
    Ok(())
}

pub(crate) fn read_active(device: &str) -> Result<Session> {
    let session = read(device)?;
    anyhow::ensure!(session.active, "Session is stopped; run the app again");
    Ok(session)
}
pub async fn active(device: &str) -> Result<Session> {
    let device = device.to_owned();
    let session = blocking(move || read_active(&device)).await?;
    check_expected(&session)?;
    Ok(session)
}
pub async fn list() -> Result<Vec<Session>> {
    blocking(|| {
        let mut result = Vec::new();
        for entry in std::fs::read_dir(root_blocking()?)? {
            let path = entry?.path();
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".session.json"))
            {
                result.push(serde_json::from_slice::<Session>(&std::fs::read(path)?)?);
            }
        }
        Ok(result)
    })
    .await
}
pub async fn bind(
    device: String,
    project: PathBuf,
    scheme: String,
    bundle_id: String,
    pid: u32,
) -> Result<Session> {
    blocking(move || {
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            device,
            project,
            scheme,
            bundle_id,
            pid,
            active: true,
            revision: 0,
            next_reference: 1,
            screen: None,
        };
        write_session(&session)?;
        Ok(session)
    })
    .await
}
pub fn check_foreground(session: &Session, screen: &Screen) -> Result<()> {
    if screen.pid.is_none() {
        anyhow::ensure!(
            unsafe { libc::kill(session.pid as i32, 0) } == 0,
            "Foreground application changed: session {} expects {} (PID {}), accessibility omitted a pid and the session process is gone",
            session.id,
            session.bundle_id,
            session.pid
        );
        return Ok(());
    }
    anyhow::ensure!(
        screen.pid == Some(session.pid),
        "Foreground application changed: session {} expects {} (PID {}), accessibility returned {:?}; refusing input",
        session.id,
        session.bundle_id,
        session.pid,
        screen.pid
    );
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Delta {
    pub session_id: String,
    pub revision: u64,
    pub base_revision: u64,
    pub full: Option<Arc<Screen>>,
    pub added: Vec<Element>,
    pub removed: Vec<u64>,
    pub changed: Vec<Element>,
}
fn keys(elements: &[Element]) -> Vec<String> {
    let mut counts = HashMap::new();
    elements
        .iter()
        .map(|e| {
            let base = serde_json::to_string(&(
                &e.role,
                &e.identifier,
                if e.identifier.is_none() {
                    e.label.as_deref()
                } else {
                    None
                },
            ))
            .unwrap();
            let count = counts.entry(base.clone()).or_insert(0usize);
            *count += 1;
            format!("{base}:{count}")
        })
        .collect()
}
pub fn update(session: &mut Session, mut screen: Screen, since: Option<u64>) -> Delta {
    let base_revision = session.revision;
    let old = session
        .screen
        .as_ref()
        .map(|s| s.elements.as_slice())
        .unwrap_or_default();
    let old_keys = keys(old);
    let previous: HashMap<_, _> = old_keys.iter().zip(old).collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    let current_keys = keys(&screen.elements);
    for (element, key) in screen.elements.iter_mut().zip(current_keys) {
        if let Some(before) = previous.get(&key) {
            element.reference = before.reference;
            if element != *before {
                changed.push(element.clone());
            }
        } else {
            element.reference = Some(session.next_reference);
            session.next_reference += 1;
            added.push(element.clone());
        }
    }
    let references: HashSet<_> = screen.elements.iter().filter_map(|e| e.reference).collect();
    let removed: Vec<_> = old
        .iter()
        .filter_map(|e| e.reference)
        .filter(|r| !references.contains(r))
        .collect();
    if session.screen.is_none() || !added.is_empty() || !changed.is_empty() || !removed.is_empty() {
        session.revision += 1;
    }
    let shared = Arc::new(screen);
    let full = if since == Some(base_revision) && session.screen.is_some() {
        None
    } else {
        Some(Arc::clone(&shared))
    };
    session.screen = Some(shared);
    Delta {
        session_id: session.id.clone(),
        revision: session.revision,
        base_revision,
        full,
        added,
        removed,
        changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample_session() -> Session {
        Session {
            id: "s".into(),
            device: "d".into(),
            project: PathBuf::new(),
            scheme: "s".into(),
            bundle_id: "b".into(),
            pid: 1,
            active: true,
            revision: 0,
            next_reference: 1,
            screen: None,
        }
    }
    fn sample_screen(label: &str) -> Screen {
        Screen {
            device: "d".into(),
            pid: Some(1),
            width: 390.0,
            height: 844.0,
            hash: None,
            elements: crate::ui::parse_elements(&format!(
                r#"[{{"type":"StaticText","AXUniqueId":"counter","AXLabel":"{label}"}}]"#
            ))
            .unwrap(),
        }
    }
    #[test]
    fn stable_references_and_delta_follow_identifier_across_label_change() {
        let mut session = sample_session();
        let mut screen = sample_screen("0");
        let first = update(&mut session, screen.clone(), None);
        screen.elements[0].label = Some("1".into());
        let changed = update(&mut session, screen.clone(), Some(first.revision));
        assert_eq!(changed.changed[0].reference, first.added[0].reference);
        assert!(changed.full.is_none());
        let unchanged = update(&mut session, screen, Some(changed.revision));
        assert!(
            unchanged.added.is_empty()
                && unchanged.changed.is_empty()
                && unchanged.removed.is_empty()
        );
        assert_eq!(unchanged.revision, changed.revision);
    }
    #[test]
    fn delta_reuses_stored_screen_allocation() {
        let mut session = sample_session();
        let screen = sample_screen("0");
        let delta = update(&mut session, screen, None);
        let stored = session.screen.as_ref().unwrap();
        let delta_screen = delta.full.as_ref().unwrap();
        assert!(Arc::ptr_eq(delta_screen, stored));
    }
    #[test]
    fn session_serializes_the_shared_screen() {
        let mut session = sample_session();
        update(&mut session, sample_screen("0"), None);
        let saved = serde_json::to_string(&session).unwrap();
        let restored: Session = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.screen, session.screen);
    }
    #[test]
    fn unsafe_identifiers_are_rejected() {
        assert!(key("../bad").is_err());
    }
}
