use crate::{process, runtime};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize)]
pub struct Usage {
    pub pid: u32,
    pub name: String,
    pub start_time: u64,
    pub physical_bytes: u64,
    pub rss_bytes: u64,
    pub cpu_ns: u64,
    pub reaped_children_cpu_ns: u64,
    pub lifetime_peak_physical_bytes: u64,
}
pub fn usage(pid: u32) -> Option<Usage> {
    if pid > i32::MAX as u32 {
        return None;
    }
    // SAFETY: rusage_info_v4 contains only integers and a byte array. libproc writes exactly this layout for flavor 4.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let status = unsafe {
        libc::proc_pid_rusage(
            pid as i32,
            4,
            (&mut info as *mut libc::rusage_info_v4).cast(),
        )
    };
    if status != 0 {
        return None;
    }
    let mut path = [0_u8; 4096];
    // SAFETY: libproc receives a writable buffer and its exact capacity.
    let length =
        unsafe { libc::proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    let name = if length > 0 {
        let end = path.iter().position(|b| *b == 0).unwrap_or(path.len());
        String::from_utf8_lossy(&path[..end])
            .rsplit('/')
            .next()
            .unwrap_or("unknown")
            .to_owned()
    } else {
        "unknown".into()
    };
    Some(Usage {
        pid,
        name,
        start_time: info.ri_proc_start_abstime,
        physical_bytes: info.ri_phys_footprint,
        rss_bytes: info.ri_resident_size,
        cpu_ns: info.ri_user_time + info.ri_system_time,
        reaped_children_cpu_ns: info.ri_child_user_time + info.ri_child_system_time,
        lifetime_peak_physical_bytes: info.ri_lifetime_max_phys_footprint,
    })
}
pub async fn snapshot(device: &str) -> Result<serde_json::Value> {
    let device = runtime::booted_device(device).await?;
    let output = process::output(
        "/bin/ps",
        &process::strings(&["-axo", "pid=,ppid=,command="]),
    )
    .await?;
    let mut parents = BTreeMap::new();
    let mut root = None;
    for line in output.lines() {
        let mut words = line.split_whitespace();
        let Some(pid) = words.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(parent) = words.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        parents.insert(pid, parent);
        if words.next() == Some("launchd_sim") && line.contains(&device.udid) {
            root = Some(pid);
        }
    }
    let mut pids =
        BTreeSet::from([root.context("Cannot locate this simulator's launchd process")?]);
    loop {
        let new: Vec<_> = parents
            .iter()
            .filter(|(pid, parent)| pids.contains(parent) && !pids.contains(pid))
            .map(|(pid, _)| *pid)
            .collect();
        if new.is_empty() {
            break;
        }
        pids.extend(new);
    }
    let mut processes: Vec<_> = pids.into_iter().filter_map(usage).collect();
    processes.sort_by_key(|p| std::cmp::Reverse(p.physical_bytes));
    Ok(
        serde_json::json!({"mx":usage(std::process::id()),"device":device.udid,"simulator_physical_bytes":processes.iter().map(|p|p.physical_bytes).sum::<u64>(),
        "simulator_process_count":processes.len(),"processes":processes,"measurement":"proc_pid_rusage v4 physical footprint; sum of per-process accounting, not unique system memory"}),
    )
}
pub async fn top(device: &str, limit: usize) -> Result<serde_json::Value> {
    anyhow::ensure!((1..=100).contains(&limit), "limit must be 1..100");
    let mut snapshot = snapshot(device).await?;
    let processes = snapshot["processes"]
        .as_array_mut()
        .context("Metrics omitted processes")?;
    processes.truncate(limit);
    snapshot["returned_process_count"] = serde_json::json!(processes.len());
    Ok(snapshot)
}
