use crate::{runtime, session, ui};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 32 * 1024;
const INDEX: &str = include_str!("web/index.html");

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub port: u16,
    pub fps: u8,
    pub quality: u8,
    pub scale: f32,
}

#[derive(Debug, Serialize)]
pub struct WebResult {
    pub device: String,
    pub session_id: String,
    pub url: String,
    pub fps: u8,
    pub quality: u8,
    pub scale: f32,
    pub first_frame_ms: u128,
}

struct FrameState {
    sequence: u64,
    frame: Arc<[u8]>,
    error: Option<String>,
}

type Frames = Arc<(Mutex<FrameState>, Condvar)>;

pub struct Server {
    result: WebResult,
    child: Child,
    group: i32,
    stopped: Arc<AtomicBool>,
    listener: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn result(&self) -> &WebResult {
        &self.result
    }

    pub async fn wait(&mut self) -> Result<()> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                anyhow::ensure!(status.success(), "AXe video stream exited with {status}");
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        unsafe {
            libc::kill(-self.group, libc::SIGTERM);
        }
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

pub async fn start(requested: &str, options: Options) -> Result<Server> {
    anyhow::ensure!((1..=30).contains(&options.fps), "fps must be 1..30");
    anyhow::ensure!(
        (1..=100).contains(&options.quality),
        "quality must be 1..100"
    );
    anyhow::ensure!(
        (0.1..=1.0).contains(&options.scale),
        "scale must be 0.1..1.0"
    );
    let device = runtime::booted_device(requested).await?;
    let bound = session::active(&device.udid).await?;
    let dimensions = ui::dimensions(&device.udid).await?;
    let listener = TcpListener::bind(("127.0.0.1", options.port))?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;

    let mut command = Command::new(ui::bridge_path());
    command
        .args([
            "stream-video",
            "--format",
            "mjpeg",
            "--fps",
            &options.fps.to_string(),
            "--quality",
            &options.quality.to_string(),
            "--scale",
            &options.scale.to_string(),
            "--udid",
            &device.udid,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.process_group(0);
    let started = Instant::now();
    let mut child = command
        .spawn()
        .context("Could not start AXe video stream")?;
    let group = child.id() as i32;
    let stdout = child.stdout.take().context("AXe stream has no stdout")?;
    let stderr = child.stderr.take().context("AXe stream has no stderr")?;
    let frames = Arc::new((
        Mutex::new(FrameState {
            sequence: 0,
            frame: Arc::from([]),
            error: None,
        }),
        Condvar::new(),
    ));
    spawn_frame_reader(stdout, Arc::clone(&frames));
    spawn_error_reader(stderr, Arc::clone(&frames));
    wait_for_first_frame(&frames, Duration::from_secs(10))?;

    let stopped = Arc::new(AtomicBool::new(false));
    let listener_thread = spawn_listener(
        listener,
        Arc::clone(&frames),
        Arc::clone(&stopped),
        device.udid.clone(),
        bound.id.clone(),
        dimensions,
    );
    Ok(Server {
        result: WebResult {
            device: device.udid,
            session_id: bound.id,
            url: format!("http://{address}"),
            fps: options.fps,
            quality: options.quality,
            scale: options.scale,
            first_frame_ms: started.elapsed().as_millis(),
        },
        child,
        group,
        stopped,
        listener: Some(listener_thread),
    })
}

fn spawn_frame_reader(mut stdout: impl Read + Send + 'static, frames: Frames) {
    thread::spawn(move || {
        let mut pending = Vec::new();
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    pending.extend_from_slice(&chunk[..count]);
                    while let Some(start) = marker(&pending, [0xff, 0xd8], 0) {
                        let Some(end) = marker(&pending, [0xff, 0xd9], start + 2) else {
                            if start > 0 {
                                pending.drain(..start);
                            }
                            break;
                        };
                        let frame = Arc::from(&pending[start..end + 2]);
                        pending.drain(..end + 2);
                        let (state, changed) = &*frames;
                        let mut state = state.lock().unwrap();
                        state.sequence += 1;
                        state.frame = frame;
                        changed.notify_all();
                    }
                    if pending.len() > MAX_FRAME_BYTES {
                        pending.clear();
                    }
                }
                Err(error) => {
                    set_error(&frames, error.to_string());
                    return;
                }
            }
        }
        set_error(&frames, "AXe video stream ended".into());
    });
}

fn spawn_error_reader(mut stderr: impl Read + Send + 'static, frames: Frames) {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.by_ref().take(8192).read_to_end(&mut bytes);
        if !bytes.is_empty() {
            set_error(&frames, String::from_utf8_lossy(&bytes).trim().to_owned());
        }
    });
}

fn marker(bytes: &[u8], marker: [u8; 2], from: usize) -> Option<usize> {
    bytes[from..]
        .windows(2)
        .position(|window| window == marker)
        .map(|index| index + from)
}

fn set_error(frames: &Frames, error: String) {
    let (state, changed) = &**frames;
    state.lock().unwrap().error = Some(error);
    changed.notify_all();
}

fn wait_for_first_frame(frames: &Frames, timeout: Duration) -> Result<()> {
    let (state, changed) = &**frames;
    let deadline = Instant::now() + timeout;
    let mut state = state.lock().unwrap();
    while state.frame.is_empty() && state.error.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("AXe produced no video frame within 10 seconds");
        }
        state = changed.wait_timeout(state, remaining).unwrap().0;
    }
    if state.frame.is_empty() {
        anyhow::bail!(
            state
                .error
                .clone()
                .unwrap_or_else(|| "AXe video stream failed".into())
        );
    }
    Ok(())
}

fn spawn_listener(
    listener: TcpListener,
    frames: Frames,
    stopped: Arc<AtomicBool>,
    device: String,
    session_id: String,
    dimensions: (f64, f64),
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let frames = Arc::clone(&frames);
                    let device = device.clone();
                    let session_id = session_id.clone();
                    thread::spawn(move || {
                        let _ = handle(stream, frames, &device, &session_id, dimensions);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    })
}

fn handle(
    mut stream: TcpStream,
    frames: Frames,
    device: &str,
    session_id: &str,
    dimensions: (f64, f64),
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request = vec![0_u8; MAX_REQUEST_BYTES];
    let count = stream.read(&mut request)?;
    let request = &request[..count];
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .context("Invalid HTTP request")?;
    let head = std::str::from_utf8(&request[..header_end])?;
    let first = head.lines().next().context("Missing request line")?;
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let body = &request[header_end + 4..];
    match (method, path) {
        ("GET", "/") => response(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            INDEX.as_bytes(),
        ),
        ("GET", "/api/status") => {
            let body = serde_json::to_vec(
                &serde_json::json!({"device":device,"session_id":session_id,"ready":true,"width":dimensions.0,"height":dimensions.1}),
            )?;
            response(&mut stream, 200, "application/json", &body)
        }
        ("GET", "/stream.mjpg") => stream_frames(stream, frames),
        ("POST", "/api/tap") => {
            control(device, session_id, body, Control::Tap)?;
            response(&mut stream, 204, "text/plain", b"")
        }
        ("POST", "/api/swipe") => {
            control(device, session_id, body, Control::Swipe)?;
            response(&mut stream, 204, "text/plain", b"")
        }
        ("POST", "/api/type") => {
            control(device, session_id, body, Control::Type)?;
            response(&mut stream, 204, "text/plain", b"")
        }
        _ => response(&mut stream, 404, "text/plain", b"Not found"),
    }
}

fn response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    Ok(())
}

fn stream_frames(mut stream: TcpStream, frames: Frames) -> Result<()> {
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n")?;
    let (state, changed) = &*frames;
    let mut sequence = 0;
    loop {
        let mut state = state.lock().unwrap();
        while state.sequence == sequence && state.error.is_none() {
            state = changed.wait(state).unwrap();
        }
        if state.sequence == sequence {
            break;
        }
        sequence = state.sequence;
        let frame = Arc::clone(&state.frame);
        drop(state);
        write!(
            stream,
            "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            frame.len()
        )?;
        stream.write_all(&frame)?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct Point {
    x: f64,
    y: f64,
}
#[derive(Deserialize)]
struct Swipe {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    duration_ms: u64,
}
#[derive(Deserialize)]
struct Text {
    text: String,
}

enum Control {
    Tap,
    Swipe,
    Type,
}

fn control(device: &str, expected_session: &str, body: &[u8], control: Control) -> Result<()> {
    let bound = session::read_active(device)?;
    anyhow::ensure!(
        bound.id == expected_session,
        "Mx session changed; reload mx web"
    );
    check_foreground(device, bound.pid)?;
    match control {
        Control::Tap => {
            let point: Point = serde_json::from_slice(body)?;
            finite(point.x, point.y)?;
            command(
                &ui::bridge_path(),
                &[
                    "tap",
                    "-x",
                    &point.x.to_string(),
                    "-y",
                    &point.y.to_string(),
                    "--udid",
                    device,
                ],
                None,
            )?;
        }
        Control::Swipe => {
            let swipe: Swipe = serde_json::from_slice(body)?;
            finite(swipe.x1, swipe.y1)?;
            finite(swipe.x2, swipe.y2)?;
            anyhow::ensure!(
                (50..=5000).contains(&swipe.duration_ms),
                "duration_ms must be 50..5000"
            );
            command(
                &ui::bridge_path(),
                &[
                    "swipe",
                    "--start-x",
                    &swipe.x1.to_string(),
                    "--start-y",
                    &swipe.y1.to_string(),
                    "--end-x",
                    &swipe.x2.to_string(),
                    "--end-y",
                    &swipe.y2.to_string(),
                    "--duration",
                    &(swipe.duration_ms as f64 / 1000.0).to_string(),
                    "--udid",
                    device,
                ],
                None,
            )?;
        }
        Control::Type => {
            let text: Text = serde_json::from_slice(body)?;
            anyhow::ensure!(text.text.len() <= 16 * 1024, "Text is too long");
            command(
                &ui::bridge_path(),
                &["type", "--stdin", "--udid", device],
                Some(text.text.as_bytes()),
            )?;
        }
    }
    Ok(())
}

fn check_foreground(device: &str, expected_pid: u32) -> Result<()> {
    let output = Command::new(ui::bridge_path())
        .args(["describe-ui", "--udid", device])
        .output()?;
    anyhow::ensure!(output.status.success(), "AXe UI inspection failed");
    let roots: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let pid = roots
        .as_array()
        .and_then(|roots| roots.first())
        .and_then(|root| root["pid"].as_u64());
    anyhow::ensure!(
        pid == Some(u64::from(expected_pid)),
        "Foreground application changed; refusing browser input"
    );
    Ok(())
}

fn finite(x: f64, y: f64) -> Result<()> {
    anyhow::ensure!(
        x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 && x <= 4096.0 && y <= 4096.0,
        "Invalid coordinates"
    );
    Ok(())
}

fn command(program: &str, args: &[&str], input: Option<&[u8]>) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input)?;
    }
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "AXe input failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
