use crate::ui::{Element, Screen};
use anyhow::{Context, Result};
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub fn root() -> Option<PathBuf> {
    std::env::var_os("MX_TEST_ROOT").map(PathBuf::from)
}

fn record(name: &str, args: &[&str]) -> Result<()> {
    let root = root().context("MX_TEST_ROOT is required for the test transport")?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("calls.jsonl"))?;
    let args = args
        .iter()
        .map(|value| format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(",");
    writeln!(file, "{{\"program\":\"{name}\",\"args\":[{args}]}}")?;
    Ok(())
}

pub fn inspect(device: &str, pid: Option<u32>) -> Result<Screen> {
    record("mxd", &["inspect", "--udid", device])?;
    let pid = std::env::var("MX_TEST_FOREGROUND_PID")
        .ok()
        .and_then(|value| value.parse().ok())
        .or(pid)
        .unwrap_or(4321);
    Ok(Screen {
        device: device.into(),
        pid: Some(pid),
        width: 390.0,
        height: 844.0,
        hash: Some("fixture".into()),
        elements: vec![
            Element {
                role: "Button".into(),
                reference: None,
                identifier: Some("first".into()),
                label: Some("Continue".into()),
                value: None,
                index: Some(0),
                frame: None,
            },
            Element {
                role: "Button".into(),
                reference: None,
                identifier: Some("second".into()),
                label: Some("Continue".into()),
                value: None,
                index: Some(1),
                frame: None,
            },
            Element {
                role: "TextField".into(),
                reference: None,
                identifier: Some("name".into()),
                label: Some("Name".into()),
                value: Some(String::new()),
                index: Some(2),
                frame: None,
            },
        ],
    })
}

pub fn tap(device: &str, selector: &crate::ui::Selector) -> Result<()> {
    let mut args = vec!["tap".to_string(), format!("--udid={device}")];
    if let Some(id) = &selector.identifier {
        args.push(format!("--id={id}"));
    }
    if let Some(label) = &selector.label {
        args.push(format!("--label={label}"));
    }
    if let Some(role) = &selector.role {
        args.push(format!("--element-type={role}"));
    }
    record("mxd", &args.iter().map(String::as_str).collect::<Vec<_>>())?;
    Ok(())
}

pub fn tap_at(device: &str, x: f64, y: f64) -> Result<()> {
    record(
        "mxd",
        &[
            "tap",
            "-x",
            &x.to_string(),
            "-y",
            &y.to_string(),
            "--udid",
            device,
        ],
    )
}

pub fn swipe(device: &str, x1: f64, y1: f64, x2: f64, y2: f64, duration_ms: u64) -> Result<()> {
    record(
        "mxd",
        &[
            "swipe",
            "--start-x",
            &x1.to_string(),
            "--start-y",
            &y1.to_string(),
            "--end-x",
            &x2.to_string(),
            "--end-y",
            &y2.to_string(),
            "--duration",
            &(duration_ms as f64 / 1000.0).to_string(),
            "--udid",
            device,
        ],
    )
}

pub fn type_text(device: &str, text: &str) -> Result<()> {
    let root = root().context("MX_TEST_ROOT is required for the test transport")?;
    record("mxd", &["type", "--stdin", "--udid", device])?;
    std::fs::write(root.join("typed.txt"), text)?;
    Ok(())
}

pub fn paste_unicode(device: &str, text: &str) -> Result<()> {
    let root = root().context("MX_TEST_ROOT is required for the test transport")?;
    record("xcrun", &["simctl", "pbcopy", device])?;
    std::fs::write(root.join("pasteboard.txt"), text)?;
    record(
        "mxd",
        &[
            "key-combo",
            "--modifiers",
            "227",
            "--key",
            "25",
            "--udid",
            device,
        ],
    )?;
    std::fs::copy(root.join("pasteboard.txt"), root.join("typed.txt"))?;
    Ok(())
}

pub fn start_video(
    device: &str,
    fps: u8,
    quality: u8,
    scale: f32,
    frames: super::video::Frames,
    stopped: Arc<AtomicBool>,
) -> Result<()> {
    record(
        "mxd",
        &[
            "stream-video",
            "--format",
            "mjpeg",
            "--fps",
            &fps.to_string(),
            "--quality",
            &quality.to_string(),
            "--scale",
            &scale.to_string(),
            "--udid",
            device,
        ],
    )?;
    thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            super::video::publish_frame(&frames, Arc::from(&b"\xff\xd8test-frame\xff\xd9"[..]));
            thread::sleep(Duration::from_millis(20));
        }
    });
    Ok(())
}
