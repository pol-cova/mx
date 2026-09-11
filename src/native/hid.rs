use anyhow::{Context, Result};
use block2::RcBlock;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2_foundation::NSError;
use std::{
    collections::HashMap,
    ffi::c_void,
    io::Write,
    process::{Command, Stdio},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::Duration,
};

// Live SimulatorKit encoding (Xcode 26 / iOS 26):
// sendWithMessage:freeWhenDone:completionQueue:completion:
//   v @{self} : ^{IndigoHIDMessageStruct=...} B @ @?
// objc2 msg_send! panics on ^v vs that struct pointer. Probe the encoding
// and call through objc_msgSend with matching ABI types.
unsafe extern "C-unwind" {
    fn objc_msgSend();
}

type SendImp = unsafe extern "C-unwind" fn(
    *mut AnyObject,
    Sel,
    *mut c_void,
    Bool,
    *mut AnyObject,
    *mut c_void,
);
type InitImp = unsafe extern "C-unwind" fn(
    *mut AnyObject,
    Sel,
    *mut AnyObject,
    *mut *mut NSError,
) -> *mut AnyObject;

#[derive(Clone, Copy)]
struct HidAbi {
    send: SendImp,
    send_sel: Sel,
    init: InitImp,
    init_sel: Sel,
}

#[derive(Clone)]
struct HidClient {
    object: Retained<AnyObject>,
}

unsafe impl Send for HidClient {}

static CLIENTS: Mutex<Option<HashMap<String, HidClient>>> = Mutex::new(None);

fn clients() -> std::sync::MutexGuard<'static, Option<HashMap<String, HidClient>>> {
    CLIENTS.lock().unwrap()
}

pub fn tap(udid: &str, x: f64, y: f64, width: f64, height: f64) -> Result<()> {
    anyhow::ensure!(
        width > 0.0 && height > 0.0,
        "Screen size is required for HID tap"
    );
    let xr = (x / width).clamp(0.0, 1.0);
    let yr = (y / height).clamp(0.0, 1.0);
    send_message(udid, &touch_message(xr, yr, true))?;
    std::thread::sleep(Duration::from_millis(50));
    send_message(udid, &touch_message(xr, yr, false))?;
    Ok(())
}

pub fn swipe(
    udid: &str,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    width: f64,
    height: f64,
    duration_ms: u64,
) -> Result<()> {
    anyhow::ensure!(
        width > 0.0 && height > 0.0,
        "Screen size is required for HID swipe"
    );
    let steps = ((duration_ms / 8).clamp(4, 40)) as usize;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let x = (x1 + (x2 - x1) * t) / width;
        let y = (y1 + (y2 - y1) * t) / height;
        send_message(
            udid,
            &touch_message(x.clamp(0.0, 1.0), y.clamp(0.0, 1.0), i < steps),
        )?;
        std::thread::sleep(Duration::from_millis((duration_ms / steps as u64).max(1)));
    }
    Ok(())
}

pub fn key_combo_cmd_v(udid: &str) -> Result<()> {
    send_message(udid, &keyboard_message(0xe3, true))?;
    send_message(udid, &keyboard_message(0x19, true))?;
    send_message(udid, &keyboard_message(0x19, false))?;
    send_message(udid, &keyboard_message(0xe3, false))?;
    Ok(())
}

pub fn type_ascii(udid: &str, text: &str) -> Result<()> {
    for ch in text.chars() {
        let Some(usage) = hid_usage(ch) else {
            anyhow::bail!("ASCII character {ch:?} has no HID usage");
        };
        send_message(udid, &keyboard_message(usage, true))?;
        send_message(udid, &keyboard_message(usage, false))?;
    }
    Ok(())
}

pub fn copy_pasteboard(udid: &str, text: &str) -> Result<()> {
    match super::coresim::set_pasteboard_text(udid, text) {
        Ok(true) => Ok(()),
        Ok(false) | Err(_) => simctl_pbcopy(udid, text),
    }
}

fn simctl_pbcopy(udid: &str, text: &str) -> Result<()> {
    let mut child = Command::new("xcrun")
        .args(["simctl", "pbcopy", udid])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start simctl pbcopy")?;
    child
        .stdin
        .take()
        .context("simctl pbcopy has no stdin")?
        .write_all(text.as_bytes())?;
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "simctl pbcopy failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn cached_client(udid: &str) -> Result<HidClient> {
    if let Some(client) = clients().as_ref().and_then(|map| map.get(udid)).cloned() {
        return Ok(client);
    }
    let created = create_client(udid)?;
    let mut map = clients();
    let map = map.get_or_insert_with(HashMap::new);
    Ok(map
        .entry(udid.into())
        .or_insert_with(|| created.clone())
        .clone())
}

fn evict(udid: &str) {
    if let Some(map) = clients().as_mut() {
        map.remove(udid);
    }
}

#[allow(dead_code)]
pub fn available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| std::panic::catch_unwind(|| hid_abi().is_ok()).unwrap_or(false))
}

fn hid_class() -> Result<&'static AnyClass> {
    AnyClass::get(c"_TtC12SimulatorKit24SimDeviceLegacyHIDClient")
        .or_else(|| AnyClass::get(c"SimDeviceLegacyHIDClient"))
        .context("SimDeviceLegacyHIDClient is unavailable")
}

fn hid_abi() -> Result<&'static HidAbi> {
    static CACHED: OnceLock<std::result::Result<HidAbi, String>> = OnceLock::new();
    CACHED
        .get_or_init(|| probe_abi().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error}"))
}

fn probe_abi() -> Result<HidAbi> {
    super::dyld::load()?;
    let class = hid_class()?;
    let send_sel = Sel::register(c"sendWithMessage:freeWhenDone:completionQueue:completion:");
    let init_sel = Sel::register(c"initWithDevice:error:");
    let send = class
        .instance_method(send_sel)
        .context("sendWithMessage:freeWhenDone:completionQueue:completion: is unavailable")?;
    anyhow::ensure!(
        send.arguments_count() == 6
            && return_code(send) == Some(b'v')
            && is_pointer(arg_bytes(send, 2).as_deref())
            && is_bool_code(arg_code(send, 3))
            && is_id_or_pointer(arg_code(send, 4))
            && is_block(arg_bytes(send, 5).as_deref()),
        "sendWithMessage:freeWhenDone:completionQueue:completion: has an unexpected type encoding"
    );
    let init = class
        .instance_method(init_sel)
        .context("initWithDevice:error: is unavailable")?;
    anyhow::ensure!(
        init.arguments_count() == 4
            && return_code(init) == Some(b'@')
            && arg_code(init, 2) == Some(b'@')
            && arg_code(init, 3) == Some(b'^'),
        "initWithDevice:error: has an unexpected type encoding"
    );
    Ok(HidAbi {
        send: unsafe {
            std::mem::transmute::<unsafe extern "C-unwind" fn(), SendImp>(objc_msgSend)
        },
        send_sel,
        init: unsafe {
            std::mem::transmute::<unsafe extern "C-unwind" fn(), InitImp>(objc_msgSend)
        },
        init_sel,
    })
}

fn return_code(method: &objc2::runtime::Method) -> Option<u8> {
    method.return_type().to_bytes().first().copied()
}

fn arg_code(method: &objc2::runtime::Method, index: usize) -> Option<u8> {
    arg_bytes(method, index)
        .as_deref()
        .and_then(|bytes| bytes.first().copied())
}

fn arg_bytes(method: &objc2::runtime::Method, index: usize) -> Option<Vec<u8>> {
    method
        .argument_type(index)
        .map(|encoding| encoding.to_bytes().to_vec())
}

fn is_pointer(bytes: Option<&[u8]>) -> bool {
    matches!(bytes.and_then(|bytes| bytes.first()), Some(&b'^' | &b'r'))
}

fn is_bool_code(code: Option<u8>) -> bool {
    matches!(code, Some(b'B' | b'c'))
}

fn is_id_or_pointer(code: Option<u8>) -> bool {
    matches!(code, Some(b'@' | b'^'))
}

fn is_block(bytes: Option<&[u8]>) -> bool {
    matches!(bytes, Some([b'@', ..] | [b'^', b'?', ..]))
}

fn send_message(udid: &str, message: &[u8]) -> Result<()> {
    let mut last_error = None;
    for _ in 0..2 {
        let client = cached_client(udid)?;
        match send(&client, message) {
            Ok(()) => return Ok(()),
            Err(error) => {
                evict(udid);
                last_error = Some(error);
            }
        }
    }
    Err(last_error.context("HID send failed")?)
}

fn create_client(udid: &str) -> Result<HidClient> {
    let abi = hid_abi()?;
    let class = hid_class()?;
    let device = super::coresim::device_by_udid(udid)?;
    let mut error: *mut NSError = std::ptr::null_mut();
    let allocated: *mut AnyObject = unsafe { msg_send![class, alloc] };
    anyhow::ensure!(!allocated.is_null(), "HID client alloc failed");
    let initialized = unsafe {
        (abi.init)(
            allocated,
            abi.init_sel,
            Retained::as_ptr(&device.object).cast_mut(),
            &mut error,
        )
    };
    anyhow::ensure!(
        !initialized.is_null(),
        if error.is_null() {
            "initWithDevice:error: returned nil".into()
        } else {
            unsafe { &*error }.localizedDescription().to_string()
        }
    );
    Ok(HidClient {
        object: unsafe { Retained::from_raw(initialized) }
            .context("HID client was not retained")?,
    })
}

fn send(client: &HidClient, message: &[u8]) -> Result<()> {
    let abi = hid_abi()?;
    let raw = unsafe { libc::malloc(message.len()) };
    anyhow::ensure!(!raw.is_null(), "HID message allocation failed");
    unsafe {
        std::ptr::copy_nonoverlapping(message.as_ptr(), raw.cast(), message.len());
    }
    let done = Arc::new((Mutex::new(None::<Option<String>>), Condvar::new()));
    let done_block = Arc::clone(&done);
    let block = RcBlock::new(move |error: *mut NSError| {
        let message = if error.is_null() {
            None
        } else {
            Some(unsafe { &*error }.localizedDescription().to_string())
        };
        let (lock, changed) = &*done_block;
        *lock.lock().unwrap() = Some(message);
        changed.notify_one();
    });
    let queue = completion_queue();
    unsafe {
        (abi.send)(
            Retained::as_ptr(&client.object).cast_mut(),
            abi.send_sel,
            raw,
            Bool::YES,
            queue,
            RcBlock::as_ptr(&block).cast(),
        );
    }
    let (lock, changed) = &*done;
    let mut state = lock.lock().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while state.is_none() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("HID send timed out");
        }
        let (next, timeout) = changed.wait_timeout(state, remaining).unwrap();
        state = next;
        if timeout.timed_out() {
            anyhow::bail!("HID send timed out");
        }
    }
    match state.take().flatten() {
        Some(error) => anyhow::bail!("HID send failed: {error}"),
        None => Ok(()),
    }
}

fn completion_queue() -> *mut AnyObject {
    const QOS_CLASS_USER_INITIATED: isize = 0x19;
    static QUEUE: OnceLock<usize> = OnceLock::new();
    let ptr = *QUEUE.get_or_init(|| {
        let serial = unsafe { dispatch_queue_create(c"mx.hid".as_ptr(), std::ptr::null()) };
        let queue = if serial.is_null() {
            unsafe { dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0) }
        } else {
            serial
        };
        queue as usize
    });
    ptr as *mut AnyObject
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const i8, attr: *const std::ffi::c_void) -> *mut AnyObject;
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut AnyObject;
}

// iOS 26 tap recipe: digitizer-style Indigo message (target 0x32). Do not
// switch to mouse NSEvent; that path mis-fires Home on current runtimes.
fn touch_message(x_ratio: f64, y_ratio: f64, down: bool) -> Vec<u8> {
    let mut message = vec![0_u8; 0x160];
    message[0x18..0x1c].copy_from_slice(&0xa0_u32.to_le_bytes());
    message[0x1c] = 0x02;
    message[0x20..0x24].copy_from_slice(&0x0b_u32.to_le_bytes());
    let timestamp = mach_absolute_time();
    message[0x24..0x2c].copy_from_slice(&timestamp.to_le_bytes());
    message[0x30..0x34].copy_from_slice(&0x400002_u32.to_le_bytes());
    message[0x34..0x38].copy_from_slice(&1_u32.to_le_bytes());
    let mask: u32 = if down { 0x07 } else { 0x06 };
    message[0x38..0x3c].copy_from_slice(&mask.to_le_bytes());
    message[0x3c..0x44].copy_from_slice(&x_ratio.to_le_bytes());
    message[0x44..0x4c].copy_from_slice(&y_ratio.to_le_bytes());
    message[0x64..0x68].copy_from_slice(&u32::from(down).to_le_bytes());
    message[0x68..0x6c].copy_from_slice(&u32::from(down).to_le_bytes());
    message[0x6c..0x70].copy_from_slice(&0x32_u32.to_le_bytes());
    // First record stays the 0xC0 digitizer layout. iOS 26 drops a lone
    // record; duplicate it at the 0xC0 wire stride and mark contact 2.
    message.copy_within(0x20..0xc0, 0xc0);
    message[0xd0..0xd4].copy_from_slice(&1_u32.to_le_bytes());
    message[0xd4..0xd8].copy_from_slice(&2_u32.to_le_bytes());
    message
}

fn keyboard_message(usage: u32, down: bool) -> Vec<u8> {
    let mut message = vec![0_u8; 0xc0];
    message[0x18..0x1c].copy_from_slice(&0xa0_u32.to_le_bytes());
    message[0x1c] = 0x01;
    message[0x20..0x24].copy_from_slice(&2_u32.to_le_bytes());
    let timestamp = mach_absolute_time();
    message[0x24..0x2c].copy_from_slice(&timestamp.to_le_bytes());
    message[0x30..0x34].copy_from_slice(&0x2710_u32.to_le_bytes());
    message[0x34..0x38].copy_from_slice(&if down { 1_u32 } else { 2_u32 }.to_le_bytes());
    message[0x38..0x3c].copy_from_slice(&0x64_u32.to_le_bytes());
    message[0x3c..0x40].copy_from_slice(&usage.to_le_bytes());
    message
}

fn hid_usage(ch: char) -> Option<u32> {
    Some(match ch {
        'a'..='z' => 0x04 + (ch as u32 - 'a' as u32),
        'A'..='Z' => 0x04 + (ch as u32 - 'A' as u32),
        '1'..='9' => 0x1e + (ch as u32 - '1' as u32),
        '0' => 0x27,
        '\n' | '\r' => 0x28,
        '\t' => 0x2b,
        ' ' => 0x2c,
        '-' => 0x2d,
        '=' => 0x2e,
        '[' => 0x2f,
        ']' => 0x30,
        '\\' => 0x31,
        ';' => 0x33,
        '\'' => 0x34,
        '`' => 0x35,
        ',' => 0x36,
        '.' => 0x37,
        '/' => 0x38,
        _ => return None,
    })
}

fn mach_absolute_time() -> u64 {
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
    }
    unsafe { mach_absolute_time() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_message_is_digitizer_indigo_layout() {
        let message = touch_message(0.5, 0.25, true);
        assert_eq!(message.len(), 0x160);
        assert_eq!(
            u32::from_le_bytes(message[0x18..0x1c].try_into().unwrap()),
            0xa0
        );
        assert_eq!(message[0x1c], 0x02);
        assert_eq!(
            f64::from_le_bytes(message[0x3c..0x44].try_into().unwrap()),
            0.5
        );
        assert_eq!(
            f64::from_le_bytes(message[0x44..0x4c].try_into().unwrap()),
            0.25
        );
        assert_eq!(
            u32::from_le_bytes(message[0x6c..0x70].try_into().unwrap()),
            0x32
        );
        assert_eq!(
            u32::from_le_bytes(message[0xd0..0xd4].try_into().unwrap()),
            1
        );
        assert_eq!(
            u32::from_le_bytes(message[0xd4..0xd8].try_into().unwrap()),
            2
        );
    }

    #[test]
    fn keyboard_message_is_button_indigo_layout() {
        let down = keyboard_message(0x04, true);
        let up = keyboard_message(0x19, false);
        assert_eq!(down.len(), 0xc0);
        assert_eq!(up.len(), 0xc0);
        assert_eq!(down[0x1c], 0x01);
        assert_eq!(up[0x1c], 0x01);
        assert_eq!(
            u32::from_le_bytes(down[0x3c..0x40].try_into().unwrap()),
            0x04
        );
        assert_eq!(u32::from_le_bytes(up[0x3c..0x40].try_into().unwrap()), 0x19);
        assert_eq!(&down[0x6c..0x70], &[0, 0, 0, 0]);
    }

    #[test]
    fn probe_and_available_do_not_panic_without_simulator_kit() {
        let _ = available();
    }

    #[test]
    fn indigo_send_probe_accepts_struct_pointer_bool_queue_and_block() {
        assert!(is_pointer(Some(b"^{IndigoHIDMessageStruct={?=IIIIIi}IC}")));
        assert!(is_pointer(Some(b"^v")));
        assert!(is_bool_code(Some(b'B')));
        assert!(is_bool_code(Some(b'c')));
        assert!(is_id_or_pointer(Some(b'@')));
        assert!(is_block(Some(b"@?")));
        assert!(!is_pointer(Some(b"@")));
        assert!(!is_bool_code(Some(b'i')));
    }
}
