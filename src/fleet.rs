use crate::{profile, session, sim};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Owned {
    pub device: String,
    pub source: String,
    pub name: String,
}
fn path(device: &str) -> Result<std::path::PathBuf> {
    anyhow::ensure!(
        device
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "Invalid device ID"
    );
    Ok(session::root()?.join(format!("{device}.owned.json")))
}
pub fn available_disk_bytes() -> Result<u64> {
    let root = session::root()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    let path = std::ffi::CString::new(root.to_string_lossy().as_bytes())?;
    // SAFETY: path is a valid C string and stats points to writable statvfs storage.
    anyhow::ensure!(
        unsafe { libc::statvfs(path.as_ptr(), &mut stats) } == 0,
        "Cannot read available disk space"
    );
    Ok(u64::from(stats.f_bavail).saturating_mul(stats.f_frsize))
}
fn require_creation_headroom() -> Result<()> {
    if std::env::var_os("MX_TEST_ROOT").is_some() {
        return Ok(());
    }
    const MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
    let available = available_disk_bytes()?;
    anyhow::ensure!(
        available >= MIN_FREE_BYTES,
        "Simulator creation requires at least 2048 MiB free; only {} MiB is available",
        available / 1024 / 1024
    );
    Ok(())
}
fn save_owned(owned: &Owned) -> Result<()> {
    use std::io::Write;
    let root = session::root()?;
    let mut temporary = tempfile::NamedTempFile::new_in(&root)?;
    serde_json::to_writer(&mut temporary, owned)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path(&owned.device)?)?;
    Ok(())
}
pub fn owned(device: &str) -> Result<Owned> {
    Ok(serde_json::from_slice(
        &std::fs::read(path(device)?).context("Mx can manage only devices created by mx_clone")?,
    )?)
}
pub async fn inventory() -> Result<serde_json::Value> {
    let devices = sim::list().await?;
    let mut owned_devices = Vec::new();
    for entry in std::fs::read_dir(session::root()?)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(device) = name.strip_suffix(".owned.json") else {
            continue;
        };
        let record = owned(device)?;
        let live = devices
            .iter()
            .find(|candidate| candidate.udid == record.device);
        owned_devices.push(serde_json::json!({
            "device": record.device,
            "source": record.source,
            "name": record.name,
            "state": live.map(|value| value.state.as_str()).unwrap_or("Unavailable"),
            "runtime": live.map(|value| value.runtime.as_str()),
            "profiled": profile::is_applied(device)
        }));
    }
    owned_devices.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(serde_json::json!({"devices": owned_devices, "count": owned_devices.len()}))
}
pub async fn clone(source: &str, name: &str) -> Result<Owned> {
    anyhow::ensure!(
        !name.trim().is_empty() && name.len() < 100,
        "Name must be 1..99 bytes"
    );
    require_creation_headroom()?;
    let source = sim::select(&sim::list().await?, Some(source))?;
    let _lock = session::lock(&source.udid).await?;
    anyhow::ensure!(
        source.state == "Shutdown",
        "Clone source must be shut down; Mx will not interrupt an existing simulator"
    );
    let device = sim::call(&["clone", &source.udid, &format!("Mx {name}")])
        .await?
        .trim()
        .to_owned();
    anyhow::ensure!(
        uuid::Uuid::parse_str(&device).is_ok(),
        "Unexpected clone UDID"
    );
    let owned = Owned {
        device,
        source: source.udid,
        name: format!("Mx {name}"),
    };
    if let Err(error) = save_owned(&owned) {
        let _ = sim::call(&["delete", &owned.device]).await;
        return Err(error).context("Clone record could not be saved; the new clone was deleted");
    }
    Ok(owned)
}
pub async fn create(template: &str, name: &str, device_type: Option<&str>) -> Result<Owned> {
    anyhow::ensure!(
        !name.trim().is_empty() && name.len() < 100,
        "Name must be 1..99 bytes"
    );
    require_creation_headroom()?;
    let template = sim::select(&sim::list().await?, Some(template))?;
    let device_type = device_type
        .map(str::to_owned)
        .or(template.device_type_identifier)
        .context("Template device type is unavailable")?;
    let device = sim::call(&[
        "create",
        &format!("Mx {name}"),
        &device_type,
        &template.runtime,
    ])
    .await?
    .trim()
    .to_owned();
    anyhow::ensure!(
        uuid::Uuid::parse_str(&device).is_ok(),
        "Unexpected created UDID"
    );
    let owned = Owned {
        device,
        source: template.udid,
        name: format!("Mx {name}"),
    };
    if let Err(error) = save_owned(&owned) {
        let _ = sim::call(&["delete", &owned.device]).await;
        return Err(error).context("Device record could not be saved; the new device was deleted");
    }
    Ok(owned)
}
pub async fn boot(
    device: &str,
    max_booted: usize,
    budget: Option<sim::MemoryBudget>,
) -> Result<sim::Device> {
    anyhow::ensure!((1..=64).contains(&max_booted), "max_booted must be 1..64");
    let owned = owned(device)?;
    let _lock = session::lock(device).await?;
    let devices = sim::list().await?;
    let selected = sim::select(&devices, Some(&owned.device))?;
    if selected.state != "Booted" {
        anyhow::ensure!(
            devices.iter().filter(|d| d.state == "Booted").count() < max_booted,
            "Boot capacity reached; shut down an idle device first"
        );
    }
    let budget = match budget {
        Some(budget) => budget,
        None => sim::default_memory_budget(
            profile::is_applied(device),
            selected.device_type_identifier.as_deref(),
        )?,
    };
    let fresh_boot = selected.state != "Booted";
    sim::boot_with_budget(&selected, max_booted, &devices, Some(budget)).await?;
    if fresh_boot {
        profile::trim_after_boot(&selected.udid).await?;
    }
    let mut result = selected;
    result.state = "Booted".into();
    Ok(result)
}
pub async fn shutdown(device: &str) -> Result<()> {
    owned(device)?;
    let _lock = session::lock(device).await?;
    if let Ok(session) = session::for_device(device).await {
        anyhow::ensure!(
            !session.active,
            "Stop the app session before shutting down its device"
        );
    }
    let selected = sim::select(&sim::list().await?, Some(device))?;
    if selected.state != "Shutdown" {
        sim::call(&["shutdown", device]).await?;
    }
    Ok(())
}
pub async fn delete(device: &str) -> Result<()> {
    owned(device)?;
    let _lock = session::lock(device).await?;
    let selected = sim::select(&sim::list().await?, Some(device))?;
    anyhow::ensure!(
        selected.state == "Shutdown",
        "Shut down the owned device before deleting it"
    );
    sim::call(&["delete", device]).await?;
    std::fs::remove_file(path(device)?)?;
    for suffix in ["session.json", "profile.json"] {
        let state = session::root()?.join(format!("{device}.{suffix}"));
        match std::fs::remove_file(state) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
