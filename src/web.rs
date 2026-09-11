use crate::{native, runtime, session};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

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

pub struct Server {
    result: WebResult,
    video: native::video::Video,
    listener: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn result(&self) -> &WebResult {
        &self.result
    }

    pub async fn wait(&mut self) -> Result<()> {
        loop {
            if self.video.stopped.load(Ordering::Relaxed) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.video.stopped.store(true, Ordering::Relaxed);
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
    let bound = session::active(&device.udid)?;
    let dimensions = native::dimensions(&device.udid).await?;
    let listener = TcpListener::bind(("127.0.0.1", options.port))?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let started = Instant::now();
    let video = native::video::start(&device.udid, options.fps, options.quality, options.scale)?;
    let stopped = Arc::clone(&video.stopped);
    let listener_thread = spawn_listener(
        listener,
        Arc::clone(&video.frames),
        stopped,
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
        video,
        listener: Some(listener_thread),
    })
}

fn spawn_listener(
    listener: TcpListener,
    frames: native::video::Frames,
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
    frames: native::video::Frames,
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
        ("GET", "/api/ui") => {
            let body = ui_tree(device)?;
            response(&mut stream, 200, "application/json", &body)
        }
        ("POST", "/api/tap") => {
            control(device, session_id, body, Control::Tap, dimensions)?;
            response(&mut stream, 204, "text/plain", b"")
        }
        ("POST", "/api/swipe") => {
            control(device, session_id, body, Control::Swipe, dimensions)?;
            response(&mut stream, 204, "text/plain", b"")
        }
        ("POST", "/api/type") => {
            control(device, session_id, body, Control::Type, dimensions)?;
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

fn stream_frames(mut stream: TcpStream, frames: native::video::Frames) -> Result<()> {
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

fn control(
    device: &str,
    expected_session: &str,
    body: &[u8],
    control: Control,
    dimensions: (f64, f64),
) -> Result<()> {
    let bound = session::active(device)?;
    anyhow::ensure!(
        bound.id == expected_session,
        "Mx session changed; reload mx web"
    );
    check_foreground(device, bound.pid)?;
    match control {
        Control::Tap => {
            let point: Point = serde_json::from_slice(body)?;
            finite(point.x, point.y)?;
            native::tap_at_blocking(device, point.x, point.y, dimensions.0, dimensions.1)?;
        }
        Control::Swipe => {
            let swipe: Swipe = serde_json::from_slice(body)?;
            finite(swipe.x1, swipe.y1)?;
            finite(swipe.x2, swipe.y2)?;
            anyhow::ensure!(
                (50..=5000).contains(&swipe.duration_ms),
                "duration_ms must be 50..5000"
            );
            native::swipe_blocking(
                device,
                swipe.x1,
                swipe.y1,
                swipe.x2,
                swipe.y2,
                swipe.duration_ms,
                dimensions.0,
                dimensions.1,
            )?;
        }
        Control::Type => {
            let text: Text = serde_json::from_slice(body)?;
            anyhow::ensure!(text.text.len() <= 16 * 1024, "Text is too long");
            native::type_text_blocking(device, &text.text)?;
        }
    }
    Ok(())
}

fn ui_tree(device: &str) -> Result<Vec<u8>> {
    let output = Command::new(ui::bridge_path())
        .args(["describe-ui", "--udid", device])
        .output()?;
    anyhow::ensure!(output.status.success(), "AXe UI inspection failed");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(serde_json::to_vec(&value)?)
}

fn check_foreground(device: &str, expected_pid: u32) -> Result<()> {
    let screen = native::inspect_blocking(device, Some(expected_pid))?;
    anyhow::ensure!(
        screen.pid == Some(expected_pid),
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
