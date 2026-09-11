use anyhow::Result;
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub struct FrameState {
    pub sequence: u64,
    pub frame: Arc<[u8]>,
    pub error: Option<String>,
}

pub type Frames = Arc<(Mutex<FrameState>, Condvar)>;

pub fn new_frames() -> Frames {
    Arc::new((
        Mutex::new(FrameState {
            sequence: 0,
            frame: Arc::from([]),
            error: None,
        }),
        Condvar::new(),
    ))
}

pub fn publish_frame(frames: &Frames, frame: Arc<[u8]>) {
    let (state, changed) = &**frames;
    let mut state = state.lock().unwrap();
    state.sequence += 1;
    state.frame = frame;
    changed.notify_all();
}

pub fn set_error(frames: &Frames, error: String) {
    let (state, changed) = &**frames;
    state.lock().unwrap().error = Some(error);
    changed.notify_all();
}

pub fn wait_for_first_frame(frames: &Frames, timeout: Duration) -> Result<()> {
    let (state, changed) = &**frames;
    let deadline = Instant::now() + timeout;
    let mut state = state.lock().unwrap();
    while state.frame.is_empty() && state.error.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("Native video produced no frame within 10 seconds");
        }
        let (next, timeout) = changed.wait_timeout(state, remaining).unwrap();
        state = next;
        if timeout.timed_out() {
            anyhow::bail!("Native video produced no frame within 10 seconds");
        }
    }
    if let Some(error) = &state.error {
        anyhow::bail!("{error}");
    }
    Ok(())
}

pub struct Video {
    pub frames: Frames,
    pub stopped: Arc<AtomicBool>,
}

impl Drop for Video {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

pub fn start(device: &str, fps: u8, quality: u8, scale: f32) -> Result<Video> {
    let frames = new_frames();
    let stopped = Arc::new(AtomicBool::new(false));
    if super::stub::root().is_some() {
        super::stub::start_video(
            device,
            fps,
            quality,
            scale,
            Arc::clone(&frames),
            Arc::clone(&stopped),
        )?;
    } else {
        let frames_thread = Arc::clone(&frames);
        let stopped_thread = Arc::clone(&stopped);
        let device = device.to_owned();
        thread::spawn(move || {
            if let Err(error) = super::screen::stream_jpegs(
                &device,
                fps,
                quality,
                scale,
                frames_thread,
                stopped_thread,
            ) {
                // The wait_for_first_frame path surfaces the first error.
                let _ = error;
            }
        });
    }
    wait_for_first_frame(&frames, Duration::from_secs(10))?;
    Ok(Video { frames, stopped })
}
