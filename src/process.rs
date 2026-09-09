use anyhow::{Context, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{os::unix::process::CommandExt, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
};

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(600);
const OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;
static SPAWN_COUNT: AtomicU64 = AtomicU64::new(0);
static ACTIVE_COUNT: AtomicU64 = AtomicU64::new(0);
static PEAK_ACTIVE_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn spawn_count() -> u64 {
    SPAWN_COUNT.load(Ordering::Relaxed)
}

pub fn peak_active_count() -> u64 {
    PEAK_ACTIVE_COUNT.load(Ordering::Relaxed)
}

struct Process {
    child: Child,
    group: i32,
}
impl Process {
    fn spawn(command: &mut Command) -> Result<Self> {
        // Each invocation owns its process group so cancellation also stops compiler children.
        command.as_std_mut().process_group(0);
        let child = command
            .kill_on_drop(true)
            .spawn()
            .context("Could not start subprocess")?;
        SPAWN_COUNT.fetch_add(1, Ordering::Relaxed);
        let active = ACTIVE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        PEAK_ACTIVE_COUNT.fetch_max(active, Ordering::Relaxed);
        let group = child.id().context("Subprocess has no PID")? as i32;
        Ok(Self { child, group })
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        ACTIVE_COUNT.fetch_sub(1, Ordering::Relaxed);
        // SAFETY: this is the positive PID of a child spawned in a new group above.
        // The negative value signals only that invocation's group, never our own.
        unsafe {
            libc::kill(-self.group, libc::SIGKILL);
        }
    }
}
fn command(program: &str, args: &[String]) -> Command {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    command
}
async fn read_limited(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    reader
        .take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut output)
        .await?;
    anyhow::ensure!(
        output.len() as u64 <= OUTPUT_LIMIT,
        "Subprocess output exceeded 8 MiB"
    );
    Ok(output)
}
fn check(
    status: std::process::ExitStatus,
    program: &str,
    stderr: &[u8],
    stdout: &[u8],
) -> Result<()> {
    if !status.success() {
        // Keep error responses compact; Xcode's complete build output has its own file.
        let tail = |bytes: &[u8]| {
            String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4096)..]).into_owned()
        };
        anyhow::bail!(
            "{program} failed ({status}):\n{}\n{}",
            tail(stderr),
            tail(stdout)
        );
    }
    Ok(())
}
pub async fn output(program: &str, args: &[String]) -> Result<String> {
    output_with_timeout(program, args, COMMAND_TIMEOUT).await
}
async fn output_with_timeout(program: &str, args: &[String], timeout: Duration) -> Result<String> {
    tokio::time::timeout(timeout, async {
        let mut process = Process::spawn(
            command(program, args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .with_context(|| format!("Could not start {program}"))?;
        let stdout = process
            .child
            .stdout
            .take()
            .context("Missing child stdout")?;
        let stderr = process
            .child
            .stderr
            .take()
            .context("Missing child stderr")?;
        let (stdout, stderr, status) =
            tokio::try_join!(read_limited(stdout), read_limited(stderr), async {
                Ok::<_, anyhow::Error>(process.child.wait().await?)
            })?;
        check(status, program, &stderr, &stdout)?;
        String::from_utf8(stdout).context("Command returned invalid UTF-8")
    })
    .await
    .with_context(|| format!("{program} exceeded {} seconds", timeout.as_secs()))?
}

pub async fn build(args: &[String], log_path: &Path) -> Result<()> {
    let log = std::fs::File::create(log_path)?;
    let mut process = Process::spawn(
        command("xcodebuild", args)
            .stdout(log.try_clone()?)
            .stderr(log),
    )?;
    let status = tokio::time::timeout(BUILD_TIMEOUT, process.child.wait())
        .await
        .with_context(|| {
            format!(
                "Build exceeded {} seconds; see {}",
                BUILD_TIMEOUT.as_secs(),
                log_path.display()
            )
        })??;
    anyhow::ensure!(
        status.success(),
        "Build failed ({status}); see {}",
        log_path.display()
    );
    Ok(())
}

pub async fn stream(program: &str, args: &[String]) -> Result<()> {
    let mut process = Process::spawn(
        command(program, args)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit()),
    )?;
    let status = process.child.wait().await?;
    check(status, program, &[], &[])
}

pub async fn input(program: &str, args: &[String], input: &[u8]) -> Result<()> {
    tokio::time::timeout(COMMAND_TIMEOUT, async {
        let mut process = Process::spawn(
            command(program, args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped()),
        )?;
        let mut stdin = process.child.stdin.take().context("Missing child stdin")?;
        let stderr = process
            .child
            .stderr
            .take()
            .context("Missing child stderr")?;
        let (_, stderr, status) = tokio::try_join!(
            async {
                stdin.write_all(input).await?;
                drop(stdin);
                Ok::<_, anyhow::Error>(())
            },
            read_limited(stderr),
            async { Ok::<_, anyhow::Error>(process.child.wait().await?) }
        )?;
        check(status, program, &stderr, &[])
    })
    .await
    .with_context(|| format!("{program} exceeded {} seconds", COMMAND_TIMEOUT.as_secs()))?
}
pub fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}

/// Drain a live process without retaining its complete output. Long lines are split at 16 KiB.
pub async fn pump(
    program: &str,
    args: &[String],
    sender: tokio::sync::mpsc::Sender<String>,
) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut process = Process::spawn(
        command(program, args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )?;
    let stdout = process.child.stdout.take().context("Missing stdout")?;
    let stderr = process.child.stderr.take().context("Missing stderr")?;
    let consume = async {
        let mut reader = BufReader::new(stdout);
        let mut pending = Vec::new();
        loop {
            let buffer = reader.fill_buf().await?;
            if buffer.is_empty() {
                break;
            }
            let count = buffer
                .iter()
                .position(|b| *b == b'\n')
                .map(|i| i + 1)
                .unwrap_or(buffer.len())
                .min(16 * 1024 - pending.len());
            pending.extend_from_slice(&buffer[..count]);
            reader.consume(count);
            if pending.last() == Some(&b'\n') || pending.len() == 16 * 1024 {
                let text = String::from_utf8_lossy(&pending)
                    .trim_end_matches('\n')
                    .to_owned();
                pending.clear();
                if sender.send(text).await.is_err() {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        }
        if !pending.is_empty() {
            let _ = sender
                .send(String::from_utf8_lossy(&pending).into_owned())
                .await;
        }
        Ok(())
    };
    let (_, stderr, status) = tokio::try_join!(consume, read_limited(stderr), async {
        Ok::<_, anyhow::Error>(process.child.wait().await?)
    })?;
    check(status, program, &stderr, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn times_out_a_stalled_process_and_its_pipe_holding_child() {
        let started = std::time::Instant::now();
        let result = output_with_timeout(
            "/bin/sh",
            &strings(&["-c", "sleep 30 & wait"]),
            Duration::from_millis(100),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("exceeded"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    #[tokio::test]
    async fn oversized_output_fails_without_deadlocking() {
        let result = output("/usr/bin/head", &strings(&["-c", "8388609", "/dev/zero"])).await;
        assert!(result.unwrap_err().to_string().contains("exceeded 8 MiB"));
    }
}
