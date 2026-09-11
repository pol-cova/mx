use crate::ui::{Element, Screen};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::PathBuf,
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
    pub screen: Option<Screen>,
}

pub fn root() -> Result<PathBuf> {
    let root = if let Some(path) = std::env::var_os("MX_STATE_DIR") {
        PathBuf::from(path)
    } else {
        PathBuf::from(std::env::var_os("HOME").context("HOME or MX_STATE_DIR is required")?)
            .join("Library/Application Support/Mx")
    };
    std::fs::create_dir_all(&root)?;
    Ok(root)
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
pub fn lock(device: &str) -> Result<File> {
    let path = root()?.join(format!("{}.lock", key(device)?));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock()
        .context("Device is busy in another Mx operation; retry after it finishes")?;
    Ok(file)
}
pub fn save(session: &Session) -> Result<()> {
    use std::io::Write;
    let root = root()?;
    let mut temp = tempfile::NamedTempFile::new_in(&root)?;
    serde_json::to_writer(&mut temp, session)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(root.join(format!("{}.session.json", key(&session.device)?)))?;
    Ok(())
}
pub fn for_device(device: &str) -> Result<Session> {
    let bytes = std::fs::read(root()?.join(format!("{}.session.json", key(device)?)))
        .context("No Mx session for this device; use mx run first")?;
    let session: Session = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(session.device == device, "Session/device mismatch");
    Ok(session)
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

pub fn active(device: &str) -> Result<Session> {
    let session = for_device(device)?;
    anyhow::ensure!(session.active, "Session is stopped; run the app again");
    check_expected(&session)?;
    Ok(session)
}
pub fn list() -> Result<Vec<Session>> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(root()?)? {
        let path = entry?.path();
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with(".session.json"))
        {
            result.push(serde_json::from_slice(&std::fs::read(path)?)?);
        }
    }
    Ok(result)
}
pub fn bind(
    device: String,
    project: PathBuf,
    scheme: String,
    bundle_id: String,
    pid: u32,
) -> Result<Session> {
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
    save(&session)?;
    Ok(session)
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
    pub full: Option<Screen>,
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
    let full = if since == Some(base_revision) && session.screen.is_some() {
        None
    } else {
        Some(screen.clone())
    };
    session.screen = Some(screen);
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
    #[test]
    fn stable_references_and_delta_follow_identifier_across_label_change() {
        let mut session = Session {
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
        };
        let mut screen = Screen {
            device: "d".into(),
            pid: Some(1),
            width: 390.0,
            height: 844.0,
            hash: None,
            elements: crate::ui::parse_elements(
                r#"[{"type":"StaticText","AXUniqueId":"counter","AXLabel":"0"}]"#,
            )
            .unwrap(),
        };
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
    fn unsafe_identifiers_are_rejected() {
        assert!(key("../bad").is_err());
    }
}
