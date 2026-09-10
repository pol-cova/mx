use crate::sim;
use anyhow::{Context, Result};

#[derive(Clone, Copy, Debug)]
pub enum Transition {
    Disable,
    Enable,
}

impl Transition {
    fn command(self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Enable => "enable",
        }
    }
}

#[derive(Debug)]
pub struct BatchResult {
    pub transitions: usize,
    pub batches: usize,
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 128
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn script(operation: Transition, labels: &[String]) -> Result<String> {
    anyhow::ensure!(
        labels.iter().all(|label| valid_label(label)),
        "Runtime catalog contains an unsafe launchd label"
    );
    let mut script =
        String::from("set -u\ntransition() {\n  label=$1\n  DYLD_ROOT_PATH=\"$SIMULATOR_ROOT\" ");
    script.push_str("\"$SIMULATOR_ROOT/bin/launchctl\" ");
    script.push_str(operation.command());
    script.push_str(
        " \"system/$label\" || {\n    /bin/sleep 0.05\n    DYLD_ROOT_PATH=\"$SIMULATOR_ROOT\" ",
    );
    script.push_str("\"$SIMULATOR_ROOT/bin/launchctl\" ");
    script.push_str(operation.command());
    script.push_str(" \"system/$label\" || { status=$?; printf 'transition failed: %s\\n' \"$label\" >&2; return \"$status\"; }\n  }\n}\npids=''\ncount=0\nfailed=0\nfor label in");
    for label in labels {
        script.push(' ');
        script.push_str(label);
    }
    script.push_str("; do\n  transition \"$label\" &\n  pids=\"$pids $!\"\n  count=$((count + 1))\n  if [ \"$count\" -eq 8 ]; then\n    for pid in $pids; do wait \"$pid\" || failed=1; done\n    [ \"$failed\" -eq 0 ] || exit 1\n    pids=''\n    count=0\n  fi\ndone\nfor pid in $pids; do wait \"$pid\" || failed=1; done\nexit \"$failed\"\n");
    anyhow::ensure!(
        script.len() <= 32 * 1024,
        "Service transition batch is too large"
    );
    Ok(script)
}

pub async fn apply(device: &str, operation: Transition, labels: &[String]) -> Result<BatchResult> {
    if labels.is_empty() {
        return Ok(BatchResult {
            transitions: 0,
            batches: 0,
        });
    }
    let script = script(operation, labels)?;
    sim::call(&["spawn", device, "/bin/sh", "-c", &script])
        .await
        .with_context(|| {
            format!(
                "Could not {} {} service transitions",
                operation.command(),
                labels.len()
            )
        })?;
    Ok(BatchResult {
        transitions: labels.len(),
        batches: 1,
    })
}

pub async fn bootout_user_agents(device: &str, labels: &[&str]) -> Result<()> {
    anyhow::ensure!(
        labels.iter().all(|label| valid_label(label)),
        "Unsafe user-agent label"
    );
    if labels.is_empty() {
        return Ok(());
    }
    let mut script = String::from("set -u\nfor label in");
    for label in labels {
        script.push(' ');
        script.push_str(label);
    }
    script.push_str("; do\n  DYLD_ROOT_PATH=\"$SIMULATOR_ROOT\" \"$SIMULATOR_ROOT/bin/launchctl\" bootout \"user/501/$label\" >/dev/null 2>&1 || true\ndone\n");
    sim::call(&["spawn", device, "/bin/sh", "-c", &script])
        .await
        .context("Could not trim optional user agents")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_script_restores_simulator_dyld_context_and_rejects_shell_input() {
        let labels = vec!["com.apple.example-service".to_owned()];
        let batch_script = script(Transition::Disable, &labels).unwrap();
        assert!(batch_script.contains("DYLD_ROOT_PATH=\"$SIMULATOR_ROOT\""));
        assert!(batch_script.contains("$SIMULATOR_ROOT/bin/launchctl"));
        assert!(batch_script.contains("com.apple.example-service"));
        assert!(script(Transition::Enable, &["com.apple.ok; reboot".into()]).is_err());
    }

    #[test]
    fn user_agent_cleanup_rejects_shell_input() {
        assert!(!valid_label("com.apple.ok; reboot"));
    }
}
