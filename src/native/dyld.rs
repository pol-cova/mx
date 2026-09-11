use anyhow::{Context, Result};
use std::{
    ffi::{CStr, CString, c_void},
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[allow(dead_code)]
pub struct Libraries {
    pub core_simulator: *mut c_void,
    pub simulator_kit: *mut c_void,
    pub apt: *mut c_void,
    pub accessibility_support: *mut c_void,
}

unsafe impl Send for Libraries {}
unsafe impl Sync for Libraries {}

pub fn developer_dir() -> Result<PathBuf> {
    static CACHED: OnceLock<PathBuf> = OnceLock::new();
    if let Some(path) = CACHED.get() {
        return Ok(path.clone());
    }
    let path = if let Ok(dir) = std::env::var("DEVELOPER_DIR") {
        PathBuf::from(dir)
    } else {
        let output = std::process::Command::new("xcode-select")
            .arg("-p")
            .output()
            .context("Could not run xcode-select")?;
        anyhow::ensure!(output.status.success(), "xcode-select -p failed");
        PathBuf::from(String::from_utf8(output.stdout)?.trim())
    };
    let _ = CACHED.set(path.clone());
    Ok(path)
}

fn dlopen_path(path: &Path) -> Option<*mut c_void> {
    let c_path = CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let handle = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
    if handle.is_null() { None } else { Some(handle) }
}

fn dlopen_first(paths: &[PathBuf]) -> *mut c_void {
    paths
        .iter()
        .find_map(|path| dlopen_path(path))
        .unwrap_or(std::ptr::null_mut())
}

pub fn load() -> Result<&'static Libraries> {
    static LOADED: OnceLock<Libraries> = OnceLock::new();
    if let Some(loaded) = LOADED.get() {
        return Ok(loaded);
    }
    let developer = developer_dir()?;
    let candidates = [
        developer.join("Library/PrivateFrameworks/CoreSimulator.framework/CoreSimulator"),
        PathBuf::from("/Library/Developer/PrivateFrameworks/CoreSimulator.framework/CoreSimulator"),
    ];
    let core_path = candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .with_context(|| {
            format!(
                "CoreSimulator.framework not found under {} or /Library/Developer/PrivateFrameworks",
                developer.display()
            )
        })?;
    let kit_candidates = [
        developer.join("Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit"),
        developer
            .parent()
            .unwrap_or(&developer)
            .join("SharedFrameworks/SimulatorKit.framework/SimulatorKit"),
        PathBuf::from("/Library/Developer/PrivateFrameworks/SimulatorKit.framework/SimulatorKit"),
    ];
    let kit = kit_candidates.iter().find(|path| path.is_file());
    let support = dlopen_first(&[
        PathBuf::from(
            "/System/Library/PrivateFrameworks/AccessibilitySupport.framework/AccessibilitySupport",
        ),
        developer
            .parent()
            .unwrap_or(&developer)
            .join("SharedFrameworks/AccessibilitySupport.framework/AccessibilitySupport"),
        PathBuf::from(
            "/Library/Developer/PrivateFrameworks/AccessibilitySupport.framework/AccessibilitySupport",
        ),
    ]);
    let apt = dlopen_first(&[
        PathBuf::from(
            "/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation",
        ),
        developer.join(
            "Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation",
        ),
        developer.parent().unwrap_or(&developer).join(
            "Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation",
        ),
        PathBuf::from(
            "/Library/Developer/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation",
        ),
    ]);
    let libraries = Libraries {
        core_simulator: dlopen_path(&core_path)
            .with_context(|| format!("Could not load {}: {}", core_path.display(), last_error()))?,
        simulator_kit: kit
            .and_then(|path| dlopen_path(path))
            .unwrap_or(std::ptr::null_mut()),
        accessibility_support: support,
        apt,
    };
    let _ = LOADED.set(libraries);
    LOADED.get().context("Native framework table missing")
}

#[allow(dead_code)]
pub fn symbol(handle: *mut c_void, name: &str) -> Option<*mut c_void> {
    if handle.is_null() {
        return None;
    }
    let name = CString::new(name).ok()?;
    let symbol = unsafe { libc::dlsym(handle, name.as_ptr()) };
    if symbol.is_null() { None } else { Some(symbol) }
}

#[allow(dead_code)]
pub fn last_error() -> String {
    let ptr = unsafe { libc::dlerror() };
    if ptr.is_null() {
        "unknown dyld error".into()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

pub fn guest_install_path() -> PathBuf {
    if let Ok(path) = std::env::var("MX_GUEST_PATH") {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("mx-guest");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    dirs_home().join("Library/Application Support/Mx/tools/mx-guest/mx-guest")
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
