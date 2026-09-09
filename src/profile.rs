use crate::{engine, fleet, process, session, sim};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Plan,
    Apply,
    Restore,
    Verify,
}
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Balanced,
    #[default]
    Slim,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub device: String,
    pub operation: Operation,
    #[serde(default)]
    pub keep: Vec<String>,
    #[serde(default)]
    pub preset: Preset,
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    device: String,
    changed: Vec<String>,
    #[serde(default)]
    keep: Vec<String>,
}
fn receipt_path(device: &str) -> Result<std::path::PathBuf> {
    Ok(session::root()?.join(format!("{device}.profile.json")))
}
pub fn is_applied(device: &str) -> bool {
    receipt_path(device).is_ok_and(|path| path.exists())
}
fn should_trim_spotlight(keep: &[String]) -> bool {
    !keep
        .iter()
        .any(|capability| matches!(capability.as_str(), "search" | "spotlight"))
}
const WATCH_USER_AGENTS: &[&str] = &[
    "com.apple.NPKCompanionAgent",
    "com.apple.addressbooksyncd",
    "com.apple.appconduitd",
    "com.apple.brook.brookcompaniond",
    "com.apple.bulletindistributord",
    "com.apple.companionappd",
    "com.apple.eventkitsyncd",
    "com.apple.nanoprefsyncd",
    "com.apple.nanoregistryd",
    "com.apple.nanoregistrylaunchd",
];
fn should_trim_watch_agents(keep: &[String]) -> bool {
    !keep.iter().any(|capability| capability == "watch")
}
pub async fn trim_after_boot(device: &str) -> Result<Vec<String>> {
    let path = receipt_path(device)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let receipt: Receipt = serde_json::from_slice(&std::fs::read(path)?)?;
    let trim_spotlight = should_trim_spotlight(&receipt.keep);
    let trim_watch = should_trim_watch_agents(&receipt.keep);
    if !trim_spotlight && !trim_watch {
        return Ok(Vec::new());
    }
    if std::env::var_os("MX_TEST_ROOT").is_none() {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    let mut trimmed = Vec::new();
    if trim_spotlight {
        match sim::call(&["terminate", device, "com.apple.Spotlight"]).await {
            Ok(_) => trimmed.push("com.apple.Spotlight".into()),
            Err(error) if error.to_string().contains("found nothing to terminate") => {}
            Err(error) => {
                return Err(error).context("Could not trim the profiled Spotlight process");
            }
        }
    }
    if trim_watch {
        engine::transaction::bootout_user_agents(device, WATCH_USER_AGENTS).await?;
        trimmed.push("watch-user-agents".into());
    }
    Ok(trimmed)
}
fn write(receipt: &Receipt) -> Result<()> {
    use std::io::Write;
    let mut temp = tempfile::NamedTempFile::new_in(session::root()?)?;
    serde_json::to_writer(&mut temp, receipt)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(receipt_path(&receipt.device)?)?;
    Ok(())
}
pub fn catalog_summary() -> serde_json::Value {
    let registry = engine::catalog::registry();
    let catalog = registry.catalog();
    serde_json::json!({
        "preset": "slim",
        "service_count": registry.service_count(),
        "categories": &catalog.categories,
        "capabilities": &catalog.features,
        "always_retained": ["network", "sharingd", "SpringBoard", "backboardd", "runningboardd", "lsd", "logd"],
        "tested_runtime": "iOS-26-5",
        "source": "Mx native runtime catalog",
        "catalog_id": "mx-runtime-ios-26.5-v1",
        "native_findings": [
            {"label":"com.apple.dmd","reason":"Starts device-management policy services in normal app test devices"},
            {"label":"com.apple.remotemanagementd","reason":"Starts a late fan-out of remote-management subscriber XPC processes"},
            {"label":"watch-user-agents","reason":"Ten optional Watch synchronization agents are removed after boot unless the watch capability is retained"}
        ]
    })
}
pub fn plan(keep: &[String], preset: Preset) -> Result<Vec<String>> {
    engine::planner::plan(
        keep,
        match preset {
            Preset::Balanced => engine::planner::Policy::Balanced,
            Preset::Slim => engine::planner::Policy::Optimized,
        },
    )
}
async fn read_disabled(device: &str) -> Result<BTreeSet<String>> {
    Ok(disabled(
        &sim::call(&["spawn", device, "launchctl", "print-disabled", "system"]).await?,
    ))
}
pub async fn status(device: &str) -> Result<serde_json::Value> {
    let selected = sim::select(&sim::list().await?, Some(device))?;
    let applied = is_applied(&selected.udid);
    let disabled = if selected.state == "Booted" {
        read_disabled(&selected.udid).await?.len()
    } else {
        0
    };
    Ok(serde_json::json!({
        "device": selected.udid,
        "state": selected.state,
        "profile_receipt": applied,
        "disabled_service_count": disabled,
        "disabled_count_available": selected.state == "Booted",
        "spotlight_trim_on_fresh_boot": applied
    }))
}
fn verify(current: &BTreeSet<String>, labels: &[String], keep: &[String]) -> Result<()> {
    let missing: Vec<_> = labels.iter().filter(|l| !current.contains(*l)).collect();
    let broken: Vec<_> = engine::planner::retained_labels(keep)?
        .into_iter()
        .filter(|label| current.contains(label))
        .collect();
    anyhow::ensure!(
        missing.is_empty() && broken.is_empty(),
        "Profile drift: {} disables missing; required services disabled: {:?}",
        missing.len(),
        broken
    );
    Ok(())
}
fn disabled(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| {
            let (label, state) = line.split_once("=>")?;
            if !matches!(state.trim().trim_end_matches(','), "true" | "disabled") {
                return None;
            }
            Some(label.trim().trim_matches('"').to_owned())
        })
        .collect()
}
pub async fn execute(request: Request) -> Result<serde_json::Value> {
    let started = std::time::Instant::now();
    let subprocess_start = process::spawn_count();
    let selected = sim::select(&sim::list().await?, Some(&request.device))?;
    let labels = plan(&request.keep, request.preset)?;
    if matches!(request.operation, Operation::Plan) {
        return Ok(
            serde_json::json!({"device":selected.udid,"disable":labels,"preset":request.preset,"service_count":labels.len(),"requires_owned_clone":true,"tested_runtime":"iOS-26-5","reboots_device":true}),
        );
    }
    if matches!(request.operation, Operation::Verify) {
        let current = read_disabled(&selected.udid).await?;
        verify(&current, &labels, &request.keep)?;
        return Ok(
            serde_json::json!({"verified":true,"device":selected.udid,"disabled_count":labels.len()}),
        );
    }
    fleet::owned(&selected.udid)?;
    let _lock = session::lock(&selected.udid)?;
    anyhow::ensure!(
        selected.runtime == "com.apple.CoreSimulator.SimRuntime.iOS-26-5",
        "This experimental capability profile is restricted to the tested iOS 26.5 runtime"
    );
    anyhow::ensure!(
        selected.state == "Booted",
        "Boot the clone before changing its profile"
    );
    if let Ok(bound) = session::for_device(&selected.udid) {
        anyhow::ensure!(
            !bound.active,
            "Stop the app session before changing its profile"
        );
    }
    let transition_started = std::time::Instant::now();
    let (transition_count, transition_batches) = match request.operation {
        Operation::Plan | Operation::Verify => unreachable!(),
        Operation::Apply => {
            let receipt_path = receipt_path(&selected.udid)?;
            let mut receipt = if receipt_path.exists() {
                let receipt: Receipt = serde_json::from_slice(&std::fs::read(&receipt_path)?)?;
                anyhow::ensure!(
                    receipt.device == selected.udid && receipt.keep == request.keep,
                    "The applied profile has different capabilities; restore it before changing capabilities"
                );
                anyhow::ensure!(
                    receipt.changed.iter().all(|label| labels.contains(label)),
                    "The applied profile contains services outside this preset; restore it before changing presets"
                );
                receipt
            } else {
                Receipt {
                    device: selected.udid.clone(),
                    changed: vec![],
                    keep: request.keep.clone(),
                }
            };
            let existing = disabled(
                &sim::call(&[
                    "spawn",
                    &selected.udid,
                    "launchctl",
                    "print-disabled",
                    "system",
                ])
                .await?,
            );
            anyhow::ensure!(
                !engine::planner::retained_labels(&request.keep)?
                    .iter()
                    .any(|label| existing.contains(label)),
                "A required service is already disabled; restore the prior profile first"
            );
            let delta: Vec<_> = labels
                .iter()
                .filter(|label| !existing.contains(*label))
                .cloned()
                .collect();
            for label in &delta {
                if !receipt.changed.contains(label) {
                    receipt.changed.push(label.clone());
                }
            }
            // The complete intended delta is durable before the first mutation. Restoring it is
            // safe after a partial batch because every member was enabled before this transaction.
            write(&receipt)?;
            let batch = engine::transaction::apply(
                &selected.udid,
                engine::transaction::Transition::Disable,
                &delta,
            )
            .await
            .context("Profile application interrupted; use restore to undo the saved receipt")?;
            (batch.transitions, batch.batches)
        }
        Operation::Restore => {
            let receipt: Receipt = serde_json::from_slice(
                &std::fs::read(receipt_path(&selected.udid)?)
                    .context("No profile receipt to restore")?,
            )?;
            let batch = engine::transaction::apply(
                &selected.udid,
                engine::transaction::Transition::Enable,
                &receipt.changed,
            )
            .await?;
            (batch.transitions, batch.batches)
        }
    };
    let transition_ms = transition_started.elapsed().as_millis();
    let reboot_started = std::time::Instant::now();
    sim::call(&["shutdown", &selected.udid]).await?;
    let mut stopped = selected.clone();
    stopped.state = "Shutdown".into();
    sim::boot_with_limit(&stopped, 64).await?;
    let actual = read_disabled(&selected.udid).await?;
    if matches!(request.operation, Operation::Apply) {
        verify(&actual, &labels, &request.keep)
            .context("Profile did not survive reboot; restore the saved receipt")?;
        trim_after_boot(&selected.udid).await?;
    }
    if matches!(request.operation, Operation::Restore) {
        let receipt: Receipt =
            serde_json::from_slice(&std::fs::read(receipt_path(&selected.udid)?)?)?;
        anyhow::ensure!(
            receipt.changed.iter().all(|l| !actual.contains(l)),
            "Restore did not survive reboot; receipt retained for retry"
        );
        std::fs::remove_file(receipt_path(&selected.udid)?)?;
    }
    Ok(serde_json::json!({
        "device": selected.udid,
        "applied": matches!(request.operation,Operation::Apply),
        "rebooted": true,
        "elapsed_ms": started.elapsed().as_millis(),
        "host_subprocesses": process::spawn_count().saturating_sub(subprocess_start),
        "peak_host_subprocesses": process::peak_active_count(),
        "transition_ms": transition_ms,
        "reboot_and_verify_ms": reboot_started.elapsed().as_millis(),
        "transition_count": transition_count,
        "transition_batches": transition_batches
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capability_plan_keeps_dependencies() {
        let labels = plan(&["photos".into(), "network".into()], Preset::Slim).unwrap();
        assert!(
            !labels
                .iter()
                .any(|s| s.contains("assetsd") || s.contains("photoanalysisd"))
        );
        assert!(plan(&["typo".into()], Preset::Slim).is_err());
        assert!(should_trim_spotlight(&[]));
        assert!(!should_trim_spotlight(&["spotlight".into()]));
        assert!(!should_trim_spotlight(&["search".into()]));
        assert!(should_trim_watch_agents(&[]));
        assert!(!should_trim_watch_agents(&["watch".into()]));
        assert_eq!(plan(&["watch".into()], Preset::Slim).unwrap().len(), 172);
    }
    #[test]
    fn preserves_existing_disabled_services() {
        assert_eq!(
            disabled("\t\"com.apple.cloudd\" => disabled\n\"com.apple.homed\" => enabled"),
            BTreeSet::from(["com.apple.cloudd".into()])
        );
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    #[test]
    fn slim_catalog_excludes_core_services_and_keeps_shared_store_dependencies() {
        let all = plan(&[], Preset::Slim).unwrap();
        assert_eq!(all.len(), 172);
        assert!(all.iter().any(|s| s == "com.apple.PosterBoard"));
        assert!(all.iter().any(|s| s == "com.apple.dmd"));
        for core in [
            "com.apple.sharingd",
            "com.apple.SpringBoard",
            "com.apple.backboardd",
            "com.apple.runningboardd",
            "com.apple.lsd",
            "com.apple.logd",
        ] {
            assert!(!all.iter().any(|s| s == core));
        }
        let kept = plan(
            &["storekit".into(), "push".into(), "universal-links".into()],
            Preset::Slim,
        )
        .unwrap();
        for service in [
            "com.apple.storekitd",
            "com.apple.amsaccountsd",
            "com.apple.passd",
            "com.apple.financed",
            "com.apple.apsd",
            "com.apple.swcd",
        ] {
            assert!(!kept.iter().any(|s| s == service));
        }
        let device_management = plan(&["device-management".into()], Preset::Slim).unwrap();
        assert!(!device_management.iter().any(|service| matches!(
            service.as_str(),
            "com.apple.dmd" | "com.apple.remotemanagementd"
        )));
    }
    #[test]
    fn reboot_verification_rejects_missing_overrides_and_disabled_requirements() {
        let labels = plan(&["push".into()], Preset::Slim).unwrap();
        let mut actual: BTreeSet<_> = labels.iter().cloned().collect();
        assert!(verify(&actual, &labels, &["push".into()]).is_ok());
        actual.insert("com.apple.apsd".into());
        assert!(verify(&actual, &labels, &["push".into()]).is_err());
        assert!(verify(&BTreeSet::new(), &labels, &[]).is_err());
    }
}
