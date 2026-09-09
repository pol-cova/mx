use crate::process::{output, strings};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub udid: String,
    pub name: String,
    pub state: String,
    pub is_available: bool,
    #[serde(default)]
    pub device_type_identifier: Option<String>,
    #[serde(default)]
    pub runtime: String,
}
#[derive(Deserialize)]
struct Inventory {
    devices: BTreeMap<String, Vec<Device>>,
}

pub fn parse_devices(json: &str) -> Result<Vec<Device>> {
    let inventory: Inventory = serde_json::from_str(json)?;
    let mut devices = Vec::new();
    for (runtime, group) in inventory.devices {
        if !runtime.starts_with("com.apple.CoreSimulator.SimRuntime.iOS-") {
            continue;
        }
        for mut device in group {
            if device.is_available {
                device.runtime = runtime.clone();
                devices.push(device);
            }
        }
    }
    devices.sort_by(|a, b| a.name.cmp(&b.name).then(a.udid.cmp(&b.udid)));
    Ok(devices)
}
pub async fn call(args: &[&str]) -> Result<String> {
    let mut command = strings(&["simctl"]);
    command.extend(strings(args));
    output("xcrun", &command).await
}
pub async fn list() -> Result<Vec<Device>> {
    parse_devices(&call(&["list", "devices", "available", "--json"]).await?)
}

pub fn select(devices: &[Device], requested: Option<&str>) -> Result<Device> {
    if let Some(id) = requested {
        let found: Vec<_> = devices
            .iter()
            .filter(|d| d.udid == id || d.name == id)
            .collect();
        return match found.as_slice() {
            [device] => Ok((*device).clone()),
            [] => anyhow::bail!("No available iOS simulator matches {id}"),
            _ => anyhow::bail!("Several simulators match {id}; specify a UDID"),
        };
    }
    let booted: Vec<_> = devices.iter().filter(|d| d.state == "Booted").collect();
    if let [device] = booted.as_slice() {
        return Ok((*device).clone());
    }
    if let [device] = devices {
        return Ok(device.clone());
    }
    bail!("Choose a simulator with --device <UDID>; use `mx devices` to list available devices")
}
pub async fn boot(device: &Device) -> Result<()> {
    boot_with_limit(device, 2).await
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryBudget {
    pub limit_mib: u64,
    /// Reserve for this next device including boot peaks; use measured peaks, not idle means.
    pub next_device_mib: u64,
}
const GENERAL_PROFILED_RESERVE_MIB: u64 = 1536;
const COMPACT_PROFILED_RESERVE_MIB: u64 = 1344;
pub const COMPACT_DEVICE_TYPE: &str =
    "com.apple.CoreSimulator.SimDeviceType.iPhone-SE-3rd-generation";

fn profiled_reserve_mib(device_type: Option<&str>) -> u64 {
    if device_type == Some(COMPACT_DEVICE_TYPE) {
        COMPACT_PROFILED_RESERVE_MIB
    } else {
        GENERAL_PROFILED_RESERVE_MIB
    }
}
impl MemoryBudget {
    pub fn allows(&self, current_bytes: u64) -> Result<()> {
        anyhow::ensure!(
            self.limit_mib > 0 && self.next_device_mib > 0 && self.limit_mib <= 1024 * 1024,
            "Invalid memory budget"
        );
        let needed = current_bytes.saturating_add(self.next_device_mib.saturating_mul(1024 * 1024));
        anyhow::ensure!(
            needed <= self.limit_mib * 1024 * 1024,
            "Simulator memory budget exceeded: current plus next-device reserve is {} MiB, budget {} MiB",
            needed / 1024 / 1024,
            self.limit_mib
        );
        Ok(())
    }
}
pub fn default_memory_budget(profiled: bool, device_type: Option<&str>) -> Result<MemoryBudget> {
    let name = std::ffi::CString::new("hw.memsize")?;
    let mut bytes = 0_u64;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: sysctlbyname receives a valid name and exact writable u64 buffer.
    anyhow::ensure!(
        unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut bytes as *mut u64).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } == 0,
        "Cannot read host memory size"
    );
    Ok(MemoryBudget {
        limit_mib: bytes / 1024 / 1024 / 2,
        next_device_mib: if profiled {
            profiled_reserve_mib(device_type)
        } else {
            4608
        },
    })
}
pub async fn capacity() -> Result<serde_json::Value> {
    let devices = list().await?;
    let booted: Vec<_> = devices
        .iter()
        .filter(|device| device.state == "Booted")
        .collect();
    let mut current_bytes = 0_u64;
    for device in &booted {
        current_bytes = current_bytes.saturating_add(
            crate::metrics::snapshot(&device.udid).await?["simulator_physical_bytes"]
                .as_u64()
                .context("Missing simulator footprint")?,
        );
    }
    let budget = default_memory_budget(true, None)?;
    let limit_bytes = budget.limit_mib.saturating_mul(1024 * 1024);
    let remaining_bytes = limit_bytes.saturating_sub(current_bytes);
    let compact_reserve_bytes = COMPACT_PROFILED_RESERVE_MIB * 1024 * 1024;
    Ok(serde_json::json!({
        "booted": booted.len(),
        "current_simulator_physical_bytes": current_bytes,
        "budget": budget,
        "estimated_additional_profiled_devices": remaining_bytes / (budget.next_device_mib * 1024 * 1024),
        "estimated_additional_compact_devices": remaining_bytes / compact_reserve_bytes,
        "available_disk_bytes": crate::fleet::available_disk_bytes()?,
        "recommended_compact_device_type": COMPACT_DEVICE_TYPE,
        "compact_profiled_device_reserve_mib": COMPACT_PROFILED_RESERVE_MIB,
        "estimate_basis": "half host RAM; 1536 MiB general or 1344 MiB measured compact-device reserve"
    }))
}
pub async fn boot_with_limit(device: &Device, max_booted: usize) -> Result<()> {
    boot_with_budget(device, max_booted, None).await
}
pub async fn boot_with_budget(
    device: &Device,
    max_booted: usize,
    budget: Option<MemoryBudget>,
) -> Result<()> {
    if device.state != "Booted" {
        let _admission = crate::session::lock("fleet-scheduler")?;
        let devices = list().await?;
        anyhow::ensure!(
            devices.iter().filter(|d| d.state == "Booted").count() < max_booted,
            "Boot capacity reached; shut down an idle simulator first"
        );
        if let Some(budget) = budget {
            let mut current = 0_u64;
            for booted in devices.iter().filter(|d| d.state == "Booted") {
                let measured = crate::metrics::snapshot(&booted.udid).await?;
                current = current.saturating_add(
                    measured["simulator_physical_bytes"]
                        .as_u64()
                        .context("Missing simulator footprint")?,
                );
            }
            budget.allows(current)?;
        }
        call(&["boot", &device.udid]).await?;
        if budget.is_some() {
            call(&["bootstatus", &device.udid, "-b"]).await?;
            return Ok(());
        }
    }
    call(&["bootstatus", &device.udid, "-b"]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_unavailable_and_non_ios() {
        let devices = parse_devices(r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-26-5":[{"udid":"1","name":"Phone","state":"Booted","isAvailable":true},{"udid":"2","name":"Old","state":"Shutdown","isAvailable":false}],"com.apple.CoreSimulator.SimRuntime.watchOS-26-5":[{"udid":"3","name":"Watch","state":"Booted","isAvailable":true}]}}"#).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(select(&devices, None).unwrap().udid, "1");
    }
    #[test]
    fn duplicate_names_require_udid() {
        let a = Device {
            udid: "1".into(),
            name: "Phone".into(),
            state: "Shutdown".into(),
            is_available: true,
            device_type_identifier: None,
            runtime: "iOS".into(),
        };
        let mut b = a.clone();
        b.udid = "2".into();
        assert!(select(&[a.clone(), b.clone()], Some("Phone")).is_err());
        assert!(select(&[a.clone(), b.clone()], None).is_err());
        assert_eq!(select(&[a, b], Some("2")).unwrap().udid, "2");
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn admission_reserves_next_devices_peak_and_rejects_invalid_budgets() {
        let budget = MemoryBudget {
            limit_mib: 4096,
            next_device_mib: 1500,
        };
        assert!(budget.allows(2000 * 1024 * 1024).is_ok());
        assert!(budget.allows(3000 * 1024 * 1024).is_err());
        assert!(
            MemoryBudget {
                limit_mib: 0,
                next_device_mib: 1
            }
            .allows(0)
            .is_err()
        );
        assert_eq!(profiled_reserve_mib(Some(COMPACT_DEVICE_TYPE)), 1344);
        assert_eq!(profiled_reserve_mib(None), 1536);
    }
}
