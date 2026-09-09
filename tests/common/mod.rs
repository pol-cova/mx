use std::{os::unix::fs::PermissionsExt, path::PathBuf};
use tempfile::TempDir;

pub struct Fixture {
    pub dir: TempDir,
    pub project: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("App with spaces ' and $signs.xcodeproj");
        std::fs::create_dir(&project).unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for name in ["xcrun", "xcodebuild", "axe"] {
            let path = bin.join(name);
            std::fs::write(&path, include_str!("tool.py")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(state.join("test-device.session.json"),serde_json::to_vec(&serde_json::json!({
            "id":"test-session","device":"test-device","project":project,"scheme":"Demo","bundle_id":"dev.mx.test","pid":4321,"active":true,"revision":0,"next_reference":1,"screen":null
        })).unwrap()).unwrap();
        Self { dir, project }
    }
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_mx"));
        command
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.dir.path().join("bin").display()),
            )
            .env("MX_TEST_ROOT", self.dir.path())
            .env("MX_STATE_DIR", self.dir.path().join("state"))
            .env("MX_AXE_PATH", self.dir.path().join("bin/axe"));
        command
    }
    pub fn calls(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.dir.path().join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
