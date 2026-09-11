mod apt;
pub(crate) mod coresim;
mod dyld;
mod encode;
mod guest;
mod hid;
mod screen;
mod stub;
pub mod video;

use crate::ui::{Screen, Selector};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub fn guest_path() -> std::path::PathBuf {
    dyld::guest_install_path()
}

pub(crate) fn using_test_transport() -> bool {
    stub::root().is_some()
}

pub fn doctor() -> Result<Value> {
    if stub::root().is_some() {
        return Ok(json!({
            "backend": "test",
            "guest_path": guest_path(),
            "screen": false,
            "ready": true
        }));
    }
    let guest = dyld::guest_install_path();
    let guest_ready = guest.is_file();
    let developer = dyld::developer_dir().ok();
    let frameworks = dyld::load();
    Ok(json!({
        "backend": "native",
        "guest_path": guest,
        "guest_ready": guest_ready,
        "developer_dir": developer,
        "core_simulator": frameworks.is_ok(),
        "core_simulator_list": coresim::list_supported(),
        "core_simulator_launch": coresim::launch_supported(),
        "core_simulator_error": frameworks.as_ref().err().map(ToString::to_string),
        "apt": apt::available(),
        "screen": screen::available(),
        "ready": guest_ready && frameworks.is_ok()
    }))
}

pub fn screenshot_png(device: &str) -> Result<Vec<u8>> {
    screen::capture_png(device)
}

pub fn inspect_blocking(device: &str, pid: Option<u32>) -> Result<Screen> {
    inspect_if_hash_not_blocking(device, pid, None)
}

pub fn inspect_if_hash_not_blocking(
    device: &str,
    pid: Option<u32>,
    if_hash_not: Option<&str>,
) -> Result<Screen> {
    if stub::root().is_some() {
        let screen = stub::inspect(device, pid)?;
        if if_hash_not.is_some() && if_hash_not == screen.hash.as_deref() {
            return Ok(Screen {
                device: screen.device,
                pid: screen.pid,
                width: screen.width,
                height: screen.height,
                hash: screen.hash,
                elements: Vec::new(),
            });
        }
        Ok(screen)
    } else {
        inspect_guest_then_apt(device, pid, if_hash_not)
    }
}

pub fn inspect_hash_blocking(device: &str, pid: Option<u32>) -> Result<Option<String>> {
    if stub::root().is_some() {
        Ok(stub::inspect(device, pid)?
            .hash
            .filter(|hash| !hash.is_empty()))
    } else {
        inspect_hash_guest_then_apt(device, pid)
    }
}

fn inspect_guest_then_apt(
    device: &str,
    pid: Option<u32>,
    if_hash_not: Option<&str>,
) -> Result<Screen> {
    match guest::inspect_detailed(device, pid, if_hash_not) {
        Ok(inspected) if inspected.unchanged => Ok(inspected.screen),
        Ok(inspected) if !inspected.screen.elements.is_empty() => Ok(inspected.screen),
        Ok(inspected) => fallback_apt(device, pid, None, inspected.screen),
        Err(error) if guest::empty_tree_error(&error) || apt::available() => fallback_apt(
            device,
            pid,
            Some(error),
            Screen {
                device: device.into(),
                pid,
                width: 0.0,
                height: 0.0,
                hash: None,
                elements: Vec::new(),
            },
        ),
        Err(error) => Err(error),
    }
}

fn inspect_hash_guest_then_apt(device: &str, pid: Option<u32>) -> Result<Option<String>> {
    match guest::inspect_hash(device, pid) {
        Ok(Some(hash)) => Ok(Some(hash)),
        Ok(None) => {
            if !apt::available() {
                return Ok(None);
            }
            Ok(apt::inspect(device, pid)?
                .hash
                .filter(|hash| !hash.is_empty()))
        }
        Err(error) => {
            if !apt::available() {
                return Err(error);
            }
            match apt::inspect(device, pid) {
                Ok(screen) => Ok(screen.hash.filter(|hash| !hash.is_empty())),
                Err(apt_error) => Err(apt_error.context(error)),
            }
        }
    }
}

fn fallback_apt(
    device: &str,
    pid: Option<u32>,
    guest_error: Option<anyhow::Error>,
    guest_screen: Screen,
) -> Result<Screen> {
    if !apt::available() {
        if let Some(error) = guest_error {
            return Err(error);
        }
        return Ok(guest_screen);
    }
    match apt::inspect(device, pid) {
        Ok(screen) => Ok(screen),
        Err(apt_error) => {
            if let Some(error) = guest_error {
                Err(apt_error.context(error))
            } else if guest_screen.elements.is_empty() {
                Err(apt_error.context("Guest snapshot returned no elements"))
            } else {
                Err(apt_error)
            }
        }
    }
}

pub async fn inspect(device: &str) -> Result<Screen> {
    inspect_pid(device, None).await
}

pub async fn inspect_pid(device: &str, pid: Option<u32>) -> Result<Screen> {
    inspect_pid_if_hash_not(device, pid, None).await
}

pub async fn inspect_pid_if_hash_not(
    device: &str,
    pid: Option<u32>,
    if_hash_not: Option<&str>,
) -> Result<Screen> {
    let device = device.to_owned();
    let if_hash_not = if_hash_not.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        inspect_if_hash_not_blocking(&device, pid, if_hash_not.as_deref())
    })
    .await?
}

pub async fn inspect_hash(device: &str, pid: Option<u32>) -> Result<Option<String>> {
    let device = device.to_owned();
    tokio::task::spawn_blocking(move || inspect_hash_blocking(&device, pid)).await?
}

pub fn tap_on_screen_blocking(screen: &Screen, selector: Selector) -> Result<()> {
    selector.validate()?;
    let matches = screen
        .elements
        .iter()
        .filter(|element| selector.matches(element))
        .count();
    if matches != 1 {
        bail!(
            "UI selector matched {matches} elements; inspect again and use a unique identifier or role"
        );
    }
    let matched = screen
        .elements
        .iter()
        .find(|element| selector.matches(element));
    if stub::root().is_some() {
        stub::tap(&screen.device, &selector)
    } else if let Some([x, y, width, height]) = matched.and_then(|element| element.frame) {
        hid::tap(
            &screen.device,
            x + width / 2.0,
            y + height / 2.0,
            screen.width,
            screen.height,
        )
    } else if let Err(press_error) = guest::press(&screen.device, screen.pid, &selector) {
        Err(press_error)
    } else {
        Ok(())
    }
}

pub async fn tap_on_screen(screen: &Screen, selector: Selector) -> Result<()> {
    let screen = screen.clone();
    tokio::task::spawn_blocking(move || tap_on_screen_blocking(&screen, selector)).await?
}

pub fn tap_at_blocking(device: &str, x: f64, y: f64, width: f64, height: f64) -> Result<()> {
    if stub::root().is_some() {
        stub::tap_at(device, x, y)
    } else {
        hid::tap(device, x, y, width, height)
    }
}

pub async fn tap_at(device: &str, x: f64, y: f64, width: f64, height: f64) -> Result<()> {
    let device = device.to_owned();
    tokio::task::spawn_blocking(move || tap_at_blocking(&device, x, y, width, height)).await?
}

pub fn swipe_blocking(
    device: &str,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    duration_ms: u64,
    width: f64,
    height: f64,
) -> Result<()> {
    if stub::root().is_some() {
        stub::swipe(device, x1, y1, x2, y2, duration_ms)
    } else {
        hid::swipe(device, x1, y1, x2, y2, width, height, duration_ms)
    }
}

pub async fn swipe(
    device: &str,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    duration_ms: u64,
    width: f64,
    height: f64,
) -> Result<()> {
    let device = device.to_owned();
    tokio::task::spawn_blocking(move || {
        swipe_blocking(&device, x1, y1, x2, y2, duration_ms, width, height)
    })
    .await?
}

pub fn type_text_blocking(device: &str, text: &str) -> Result<()> {
    anyhow::ensure!(
        text.len() <= 16 * 1024,
        "Text input is limited to 16 KiB per call"
    );
    anyhow::ensure!(
        text.chars()
            .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t')),
        "Text contains unsupported control characters"
    );
    if stub::root().is_some() {
        if text.is_ascii() {
            stub::type_text(device, text)
        } else {
            stub::paste_unicode(device, text)
        }
    } else if text.is_ascii() {
        hid::type_ascii(device, text)
    } else {
        hid::copy_pasteboard(device, text)?;
        hid::key_combo_cmd_v(device)
    }
}

pub async fn type_text(device: &str, text: &str) -> Result<()> {
    let device = device.to_owned();
    let text = text.to_owned();
    tokio::task::spawn_blocking(move || type_text_blocking(&device, &text)).await?
}

pub async fn dimensions(device: &str) -> Result<(f64, f64)> {
    let screen = inspect(device).await?;
    anyhow::ensure!(
        screen.width > 0.0 && screen.height > 0.0,
        "Native snapshot returned invalid screen dimensions"
    );
    Ok((screen.width, screen.height))
}
