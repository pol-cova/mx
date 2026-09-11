use anyhow::{Context, Result};
use block2::RcBlock;
use jpeg_encoder::{ColorType, Encoder};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::NSUUID;
use std::{
    collections::HashMap,
    ffi::c_void,
    process::Command,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const LOCK_READ_ONLY: u32 = 1;
const FOURCC_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const FOURCC_RGBA: u32 = u32::from_be_bytes(*b"RGBA");
const FOURCC_ARGB: u32 = u32::from_be_bytes(*b"ARGB");

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceGetWidth(buffer: *mut c_void) -> usize;
    fn IOSurfaceGetHeight(buffer: *mut c_void) -> usize;
    fn IOSurfaceGetBytesPerRow(buffer: *mut c_void) -> usize;
    fn IOSurfaceGetBaseAddress(buffer: *mut c_void) -> *mut c_void;
    fn IOSurfaceGetPixelFormat(buffer: *mut c_void) -> u32;
    fn IOSurfaceLock(buffer: *mut c_void, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(buffer: *mut c_void, options: u32, seed: *mut u32) -> i32;
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const libc::c_char, attr: *const c_void) -> *mut AnyObject;
}

struct FrameClock {
    counter: AtomicU64,
    last_nanos: AtomicU64,
    lock: Mutex<()>,
    changed: Condvar,
}

impl FrameClock {
    fn new() -> Self {
        Self {
            counter: AtomicU64::new(0),
            last_nanos: AtomicU64::new(0),
            lock: Mutex::new(()),
            changed: Condvar::new(),
        }
    }

    fn bump(&self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
        self.last_nanos.store(nanos_now(), Ordering::SeqCst);
        let _guard = self.lock.lock().unwrap();
        self.changed.notify_all();
    }

    fn current(&self) -> u64 {
        self.counter.load(Ordering::SeqCst)
    }

    fn wait_after(&self, last: u64, timeout: Option<Duration>) -> u64 {
        let mut guard = self.lock.lock().unwrap();
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            let now = self.current();
            if now > last {
                return now;
            }
            match deadline {
                None => {
                    guard = self.changed.wait(guard).unwrap();
                }
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return self.current();
                    }
                    let (next, timed) = self.changed.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if timed.timed_out() {
                        return self.current();
                    }
                }
            }
        }
    }
}

struct Subscription {
    descriptors: Vec<Retained<AnyObject>>,
    uuids: Vec<Retained<NSUUID>>,
    clock: Arc<FrameClock>,
    pixels: Mutex<Vec<u8>>,
    _device: super::coresim::SimDevice,
    _queue: Retained<AnyObject>,
    _blocks: Vec<RcBlock<dyn Fn()>>,
}

unsafe impl Send for Subscription {}
unsafe impl Sync for Subscription {}

impl Drop for Subscription {
    fn drop(&mut self) {
        let unregister = super::coresim::unregister_sel();
        for (descriptor, uuid) in self.descriptors.iter().zip(&self.uuids) {
            let responds: bool =
                unsafe { msg_send![&**descriptor, respondsToSelector: unregister] };
            if responds {
                let _: () =
                    unsafe { msg_send![&**descriptor, unregisterScreenCallbacksWithUUID: &**uuid] };
            }
        }
    }
}

fn nanos_now() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

fn subscriptions() -> std::sync::MutexGuard<'static, HashMap<String, Arc<Subscription>>> {
    static SUBS: OnceLock<Mutex<HashMap<String, Arc<Subscription>>>> = OnceLock::new();
    SUBS.get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
}

pub fn available() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| std::panic::catch_unwind(probe_screen).unwrap_or(false))
}

fn probe_screen() -> bool {
    let Ok(libs) = super::dyld::load() else {
        return false;
    };
    if libs.simulator_kit.is_null() {
        return false;
    }
    super::coresim::screen_selectors_present()
}

pub fn subscribe(udid: &str) -> Result<()> {
    if subscriptions().contains_key(udid) {
        return Ok(());
    }
    let created = create_subscription(udid)?;
    subscriptions().insert(udid.to_owned(), created);
    Ok(())
}

pub fn current_counter(udid: &str) -> u64 {
    subscriptions()
        .get(udid)
        .map(|subscription| subscription.clock.current())
        .unwrap_or(0)
}

pub fn frames_since(udid: &str, last: u64) -> u64 {
    current_counter(udid).saturating_sub(last)
}

#[allow(dead_code)]
pub fn last_frame_nanos(udid: &str) -> u64 {
    subscriptions()
        .get(udid)
        .map(|subscription| subscription.clock.last_nanos.load(Ordering::SeqCst))
        .unwrap_or(0)
}

pub fn wait_frame_after(udid: &str, last: u64, timeout: Option<Duration>) -> Result<u64> {
    let clock = subscriptions()
        .get(udid)
        .map(|subscription| Arc::clone(&subscription.clock))
        .context("No framebuffer subscription")?;
    Ok(clock.wait_after(last, timeout))
}

fn create_subscription(udid: &str) -> Result<Arc<Subscription>> {
    super::dyld::load()?;
    anyhow::ensure!(available(), "SimulatorKit framebuffer is unavailable");
    let device = super::coresim::device_by_udid(udid)?;
    let descriptors = super::coresim::framebuffer_descriptors(&device)?;
    let register = super::coresim::register_sel();
    let label = std::ffi::CString::new(format!("dev.mx.screen.{udid}"))
        .context("Framebuffer queue label is invalid")?;
    let queue_ptr = unsafe { dispatch_queue_create(label.as_ptr(), std::ptr::null()) };
    anyhow::ensure!(!queue_ptr.is_null(), "dispatch_queue_create failed");
    let queue = unsafe { Retained::from_raw(queue_ptr) }
        .context("Framebuffer callback queue was not retained")?;
    let clock = Arc::new(FrameClock::new());
    let mut uuids = Vec::new();
    let mut blocks = Vec::new();
    let mut registered = Vec::new();
    for descriptor in descriptors {
        let responds: bool = unsafe { msg_send![&*descriptor, respondsToSelector: register] };
        if !responds {
            continue;
        }
        let uuid = NSUUID::UUID();
        let frame_clock = Arc::clone(&clock);
        let surfaces_clock = Arc::clone(&clock);
        let frame = RcBlock::new(move || frame_clock.bump());
        let surfaces = RcBlock::new(move || surfaces_clock.bump());
        let properties = RcBlock::new(|| {});
        let _: () = unsafe {
            msg_send![
                &*descriptor,
                registerScreenCallbacksWithUUID: &*uuid,
                callbackQueue: &*queue,
                frameCallback: &*frame,
                surfacesChangedCallback: &*surfaces,
                propertiesChangedCallback: &*properties
            ]
        };
        uuids.push(uuid);
        blocks.push(frame);
        blocks.push(surfaces);
        blocks.push(properties);
        registered.push(descriptor);
    }
    anyhow::ensure!(
        !registered.is_empty(),
        "registerScreenCallbacks is unavailable on framebuffer descriptors"
    );
    Ok(Arc::new(Subscription {
        descriptors: registered,
        uuids,
        clock,
        pixels: Mutex::new(Vec::new()),
        _device: device,
        _queue: queue,
        _blocks: blocks,
    }))
}

pub fn capture_png(device: &str) -> Result<Vec<u8>> {
    if try_surface(device) {
        if let Ok(bytes) = capture_surface(device, Output::Png, 100, 1.0) {
            return Ok(bytes);
        }
    }
    capture_png_simctl(device)
}

pub fn stream_jpegs(
    device: &str,
    fps: u8,
    quality: u8,
    scale: f32,
    frames: super::video::Frames,
    stopped: Arc<AtomicBool>,
) -> Result<()> {
    let interval = Duration::from_millis((1000 / u64::from(fps.max(1))).max(1));
    let mut native = try_surface(device);
    let mut last = current_counter(device);
    let mut last_emit: Option<Instant> = None;
    while !stopped.load(Ordering::Relaxed) {
        if native {
            if let Some(emitted) = last_emit {
                let remaining = interval.saturating_sub(emitted.elapsed());
                if !remaining.is_zero() {
                    if frames_since(device, last) == 0 {
                        let _ = wait_frame_after(device, last, Some(remaining));
                    }
                    let remaining = interval.saturating_sub(emitted.elapsed());
                    if !remaining.is_zero() {
                        std::thread::sleep(remaining);
                    }
                }
            }
            last = current_counter(device);
        }
        let jpeg = if native {
            match capture_surface(device, Output::Jpeg, quality, scale) {
                Ok(jpeg) => jpeg,
                Err(error) if last_emit.is_none() => {
                    native = false;
                    capture_jpeg_simctl(device, quality, scale).map_err(|_| error)?
                }
                Err(error) => {
                    super::video::set_error(&frames, error.to_string());
                    return Err(error);
                }
            }
        } else {
            capture_jpeg_simctl(device, quality, scale)?
        };
        super::video::publish_frame(&frames, Arc::from(jpeg.as_slice()));
        last_emit = Some(Instant::now());
        if !native {
            std::thread::sleep(interval);
        }
    }
    Ok(())
}

fn try_surface(device: &str) -> bool {
    available() && subscribe(device).is_ok()
}

#[derive(Clone, Copy)]
enum Output {
    Jpeg,
    Png,
}

fn capture_surface(device: &str, output: Output, quality: u8, scale: f32) -> Result<Vec<u8>> {
    let subscription = subscriptions()
        .get(device)
        .cloned()
        .context("No framebuffer subscription")?;
    let surface =
        largest_surface(&subscription.descriptors).context("framebufferSurface is nil")?;
    let locked = unsafe { IOSurfaceLock(surface, LOCK_READ_ONLY, std::ptr::null_mut()) };
    anyhow::ensure!(locked == 0, "IOSurfaceLock failed ({locked})");
    let encoded = encode_locked_surface(surface, output, quality, scale, &subscription.pixels);
    let _ = unsafe { IOSurfaceUnlock(surface, LOCK_READ_ONLY, std::ptr::null_mut()) };
    encoded
}

fn largest_surface(descriptors: &[Retained<AnyObject>]) -> Option<*mut c_void> {
    let sel = super::coresim::framebuffer_surface_sel();
    let mut best = std::ptr::null_mut();
    let mut best_area = 0_usize;
    for descriptor in descriptors {
        let responds: bool = unsafe { msg_send![&**descriptor, respondsToSelector: sel] };
        if !responds {
            continue;
        }
        let surface: *mut c_void = unsafe { msg_send![&**descriptor, framebufferSurface] };
        if surface.is_null() {
            continue;
        }
        let area =
            unsafe { IOSurfaceGetWidth(surface).saturating_mul(IOSurfaceGetHeight(surface)) };
        if area > best_area {
            best = surface;
            best_area = area;
        }
    }
    if best.is_null() { None } else { Some(best) }
}

fn encode_locked_surface(
    surface: *mut c_void,
    output: Output,
    quality: u8,
    scale: f32,
    pixels: &Mutex<Vec<u8>>,
) -> Result<Vec<u8>> {
    let width = unsafe { IOSurfaceGetWidth(surface) };
    let height = unsafe { IOSurfaceGetHeight(surface) };
    let bytes_per_row = unsafe { IOSurfaceGetBytesPerRow(surface) };
    let format = unsafe { IOSurfaceGetPixelFormat(surface) };
    let base = unsafe { IOSurfaceGetBaseAddress(surface) }.cast::<u8>();
    anyhow::ensure!(
        !base.is_null() && width > 0 && height > 0,
        "IOSurface has no pixels"
    );
    anyhow::ensure!(
        bytes_per_row >= width.saturating_mul(4),
        "IOSurface bytesPerRow {bytes_per_row} is smaller than width {width}"
    );
    let src = unsafe { std::slice::from_raw_parts(base, height.saturating_mul(bytes_per_row)) };
    let mut packed = pixels.lock().unwrap();
    let (out_w, out_h) = pack_to_rgb(
        src,
        width as u32,
        height as u32,
        bytes_per_row,
        format,
        scale,
        &mut packed,
    )?;
    match output {
        Output::Jpeg => encode_rgb_jpeg(&packed, out_w, out_h, quality),
        Output::Png => encode_rgb_png(&packed, out_w, out_h),
    }
}

#[derive(Clone, Copy)]
enum PixelFormat {
    Bgra,
    Rgba,
    Argb,
}

fn pixel_format(fourcc: u32) -> PixelFormat {
    if fourcc == FOURCC_RGBA {
        PixelFormat::Rgba
    } else if fourcc == FOURCC_ARGB {
        PixelFormat::Argb
    } else if fourcc == FOURCC_BGRA {
        PixelFormat::Bgra
    } else {
        PixelFormat::Bgra
    }
}

fn pack_to_rgb(
    src: &[u8],
    width: u32,
    height: u32,
    bytes_per_row: usize,
    fourcc: u32,
    scale: f32,
    out: &mut Vec<u8>,
) -> Result<(u32, u32)> {
    anyhow::ensure!(width > 0 && height > 0, "Frame size must be positive");
    anyhow::ensure!(
        bytes_per_row >= width as usize * 4,
        "bytesPerRow {bytes_per_row} is smaller than width {width}"
    );
    let needed = height as usize * bytes_per_row;
    anyhow::ensure!(
        src.len() >= needed,
        "BGRA buffer is shorter than bytesPerRow * height"
    );
    let scale = if scale <= 0.0 {
        1.0
    } else {
        scale.clamp(0.1, 1.0)
    };
    let out_w = ((width as f32) * scale).round().max(1.0) as u32;
    let out_h = ((height as f32) * scale).round().max(1.0) as u32;
    out.clear();
    out.resize(out_w as usize * out_h as usize * 3, 0);
    let format = pixel_format(fourcc);
    for y in 0..out_h {
        let src_y = (y as u64 * height as u64 / out_h as u64) as usize;
        let row = &src[src_y * bytes_per_row..src_y * bytes_per_row + width as usize * 4];
        for x in 0..out_w {
            let src_x = (x as u64 * width as u64 / out_w as u64) as usize;
            let pixel = &row[src_x * 4..src_x * 4 + 4];
            let (r, g, b) = match format {
                PixelFormat::Bgra => (pixel[2], pixel[1], pixel[0]),
                PixelFormat::Rgba => (pixel[0], pixel[1], pixel[2]),
                PixelFormat::Argb => (pixel[1], pixel[2], pixel[3]),
            };
            let dst = ((y * out_w + x) * 3) as usize;
            out[dst] = r;
            out[dst + 1] = g;
            out[dst + 2] = b;
        }
    }
    Ok((out_w, out_h))
}

#[cfg(test)]
fn encode_bgra_jpeg(
    bgra: &[u8],
    width: u32,
    height: u32,
    bytes_per_row: usize,
    quality: u8,
    scale: f32,
) -> Result<Vec<u8>> {
    let mut rgb = Vec::new();
    let (width, height) = pack_to_rgb(
        bgra,
        width,
        height,
        bytes_per_row,
        FOURCC_BGRA,
        scale,
        &mut rgb,
    )?;
    encode_rgb_jpeg(&rgb, width, height, quality)
}

#[cfg(test)]
fn encode_bgra_png(
    bgra: &[u8],
    width: u32,
    height: u32,
    bytes_per_row: usize,
    scale: f32,
) -> Result<Vec<u8>> {
    let mut rgb = Vec::new();
    let (width, height) = pack_to_rgb(
        bgra,
        width,
        height,
        bytes_per_row,
        FOURCC_BGRA,
        scale,
        &mut rgb,
    )?;
    encode_rgb_png(&rgb, width, height)
}

fn encode_rgb_jpeg(rgb: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>> {
    anyhow::ensure!(
        width <= u16::MAX as u32 && height <= u16::MAX as u32,
        "Frame is too large"
    );
    let mut out = Vec::new();
    Encoder::new(&mut out, quality.clamp(1, 100))
        .encode(rgb, width as u16, height as u16, ColorType::Rgb)
        .map_err(|error| anyhow::anyhow!("JPEG encode failed: {error}"))?;
    Ok(out)
}

fn encode_rgb_png(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().context("PNG header encode failed")?;
    writer
        .write_image_data(rgb)
        .context("PNG image encode failed")?;
    writer.finish().context("PNG finish failed")?;
    Ok(out)
}

fn capture_jpeg_simctl(device: &str, quality: u8, scale: f32) -> Result<Vec<u8>> {
    let dir = std::env::temp_dir();
    let jpeg_path = dir.join(format!("mx-{device}-frame.jpg"));
    let png_path = dir.join(format!("mx-{device}-frame.png"));
    let jpeg_status = Command::new("xcrun")
        .args(["simctl", "io", device, "screenshot", "--type=jpeg"])
        .arg(&jpeg_path)
        .output()
        .context("Could not run simctl screenshot")?;
    if jpeg_status.status.success() {
        if let Ok(bytes) = std::fs::read(&jpeg_path) {
            if bytes.len() > 2 && bytes[0] == 0xff && bytes[1] == 0xd8 {
                let _ = scale;
                return Ok(bytes);
            }
        }
    }
    let png_status = Command::new("xcrun")
        .args(["simctl", "io", device, "screenshot"])
        .arg(&png_path)
        .output()
        .context("Could not run simctl screenshot")?;
    anyhow::ensure!(
        png_status.status.success(),
        "simctl screenshot failed: {}",
        String::from_utf8_lossy(&png_status.stderr)
    );
    let converted = Command::new("sips")
        .args(["-s", "format", "jpeg", "-s", "formatOptions"])
        .arg(quality.clamp(1, 100).to_string())
        .arg(&png_path)
        .args(["--out"])
        .arg(&jpeg_path)
        .output()
        .context("Could not convert screenshot to JPEG")?;
    anyhow::ensure!(
        converted.status.success(),
        "sips JPEG conversion failed: {}",
        String::from_utf8_lossy(&converted.stderr)
    );
    std::fs::read(&jpeg_path).context("Could not read converted screenshot JPEG")
}

fn capture_png_simctl(device: &str) -> Result<Vec<u8>> {
    let dir = std::env::temp_dir();
    let png_path = dir.join(format!("mx-{device}-frame.png"));
    let png_status = Command::new("xcrun")
        .args(["simctl", "io", device, "screenshot", "--type=png"])
        .arg(&png_path)
        .output()
        .context("Could not run simctl screenshot")?;
    anyhow::ensure!(
        png_status.status.success(),
        "simctl screenshot failed: {}",
        String::from_utf8_lossy(&png_status.stderr)
    );
    std::fs::read(&png_path).context("Could not read simctl screenshot PNG")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn padded_bgra() -> (Vec<u8>, u32, u32, usize) {
        let width = 2_u32;
        let height = 2_u32;
        let bytes_per_row = 16_usize;
        let mut bgra = vec![0_u8; bytes_per_row * height as usize];
        // Opaque red, green / blue, white in BGRA, with 8 bytes of row padding.
        bgra[0..4].copy_from_slice(&[0, 0, 255, 255]);
        bgra[4..8].copy_from_slice(&[0, 255, 0, 255]);
        bgra[16..20].copy_from_slice(&[255, 0, 0, 255]);
        bgra[20..24].copy_from_slice(&[255, 255, 255, 255]);
        (bgra, width, height, bytes_per_row)
    }

    #[test]
    fn pack_skips_row_padding() {
        let (bgra, width, height, bytes_per_row) = padded_bgra();
        let mut rgb = Vec::new();
        let (out_w, out_h) = pack_to_rgb(
            &bgra,
            width,
            height,
            bytes_per_row,
            FOURCC_BGRA,
            1.0,
            &mut rgb,
        )
        .unwrap();
        assert_eq!((out_w, out_h), (2, 2));
        assert_eq!(rgb, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    }

    #[test]
    fn encodes_synthetic_bgra_jpeg() {
        let (bgra, width, height, bytes_per_row) = padded_bgra();
        let jpeg = encode_bgra_jpeg(&bgra, width, height, bytes_per_row, 80, 1.0).unwrap();
        assert!(jpeg.len() > 2 && jpeg[0] == 0xff && jpeg[1] == 0xd8);
    }

    #[test]
    fn encodes_synthetic_bgra_png() {
        let (bgra, width, height, bytes_per_row) = padded_bgra();
        let png = encode_bgra_png(&bgra, width, height, bytes_per_row, 1.0).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    #[test]
    fn probe_and_clock_do_not_panic_without_a_simulator() {
        let _ = available();
        assert_eq!(current_counter("missing-udid"), 0);
        assert_eq!(frames_since("missing-udid", 4), 0);
        assert_eq!(last_frame_nanos("missing-udid"), 0);
        assert!(wait_frame_after("missing-udid", 0, Some(Duration::from_millis(1))).is_err());
        let doctor = crate::native::doctor().unwrap();
        assert!(
            doctor
                .get("screen")
                .and_then(|value| value.as_bool())
                .is_some()
        );
    }

    #[test]
    fn pack_scales_down_bgra() {
        let (bgra, width, height, bytes_per_row) = padded_bgra();
        let mut rgb = Vec::new();
        let (out_w, out_h) = pack_to_rgb(
            &bgra,
            width,
            height,
            bytes_per_row,
            FOURCC_BGRA,
            0.5,
            &mut rgb,
        )
        .unwrap();
        assert_eq!((out_w, out_h), (1, 1));
        assert_eq!(rgb, vec![255, 0, 0]);
    }
}
