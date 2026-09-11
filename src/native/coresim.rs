use anyhow::{Context, Result};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{Message, class, msg_send};
use objc2_foundation::{NSArray, NSError, NSString};
use std::ffi::{CStr, CString, c_void};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct SimDevice {
    pub object: Retained<AnyObject>,
}

unsafe impl Send for SimDevice {}
unsafe impl Sync for SimDevice {}

#[derive(Debug, Clone)]
pub struct ListedDevice {
    pub udid: String,
    pub name: String,
    pub state: String,
    pub device_type: String,
    pub runtime: String,
    pub available: bool,
}

pub fn device_by_udid(udid: &str) -> Result<SimDevice> {
    let devices: Retained<NSArray<AnyObject>> = unsafe { msg_send![&*device_set()?, devices] };
    for device in devices.iter() {
        if udid_of(&device) == udid {
            return Ok(SimDevice {
                object: device.clone(),
            });
        }
    }
    anyhow::bail!("CoreSimulator has no device {udid}")
}

pub fn list_devices() -> Result<Vec<ListedDevice>> {
    anyhow::ensure!(
        list_supported(),
        "CoreSimulator device listing is unavailable"
    );
    let devices: Retained<NSArray<AnyObject>> = unsafe { msg_send![&*device_set()?, devices] };
    Ok(devices
        .iter()
        .map(|device| ListedDevice {
            udid: udid_of(&device),
            name: string_prop(&device, |object| unsafe { msg_send![object, name] }),
            state: state_of(&device),
            device_type: string_prop(&device, |object| unsafe {
                msg_send![object, deviceTypeIdentifier]
            }),
            runtime: string_prop(&device, |object| unsafe {
                msg_send![object, runtimeIdentifier]
            }),
            available: available_of(&device),
        })
        .collect())
}

pub fn list_supported() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| probe_list().is_ok())
}

pub fn launch_supported() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| probe_launch().is_ok())
}

pub fn terminate_and_launch(udid: &str, bundle_id: &str) -> Result<u32> {
    anyhow::ensure!(
        launch_supported(),
        "CoreSimulator launch selectors are unavailable"
    );
    let device = device_by_udid(udid)?;
    let bundle = NSString::from_str(bundle_id);
    let mut error: *mut NSError = std::ptr::null_mut();
    let _: bool = unsafe {
        msg_send![
            &*device.object,
            terminateApplicationWithID: &*bundle,
            error: &mut error
        ]
    };
    let options: Retained<AnyObject> = unsafe { msg_send![class!(NSDictionary), dictionary] };
    let mut error: *mut NSError = std::ptr::null_mut();
    let pid: i32 = unsafe {
        msg_send![
            &*device.object,
            launchApplicationWithID: &*bundle,
            options: &*options,
            error: &mut error
        ]
    };
    anyhow::ensure!(
        pid > 0,
        ns_error(error, "CoreSimulator launch returned no PID")
    );
    Ok(pid as u32)
}

#[allow(dead_code)]
pub fn data_tmp(udid: &str) -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join("Library/Developer/CoreSimulator/Devices")
        .join(udid)
        .join("data/tmp")
}

fn probe_list() -> Result<()> {
    super::dyld::load()?;
    let device = sim_device_class()?;
    anyhow::ensure!(
        method_returns(device, c"UDID", b'@'),
        "SimDevice UDID is unavailable"
    );
    anyhow::ensure!(
        method_returns(device, c"name", b'@'),
        "SimDevice name is unavailable"
    );
    anyhow::ensure!(
        method_returns(device, c"deviceTypeIdentifier", b'@'),
        "SimDevice deviceTypeIdentifier is unavailable"
    );
    anyhow::ensure!(
        method_returns(device, c"runtimeIdentifier", b'@'),
        "SimDevice runtimeIdentifier is unavailable"
    );
    anyhow::ensure!(
        method_returns(device, c"stateString", b'@') || method_returns(device, c"state", b'Q'),
        "SimDevice state is unavailable"
    );
    let set = AnyClass::get(c"SimDeviceSet").context("SimDeviceSet is unavailable")?;
    anyhow::ensure!(
        method_returns(set, c"devices", b'@'),
        "SimDeviceSet devices is unavailable"
    );
    Ok(())
}

fn probe_launch() -> Result<()> {
    super::dyld::load()?;
    let device = sim_device_class()?;
    let launch = device
        .instance_method(Sel::register(c"launchApplicationWithID:options:error:"))
        .context("SimDevice launchApplicationWithID:options:error: is unavailable")?;
    anyhow::ensure!(
        launch.return_type().to_bytes().first() == Some(&b'i') && launch.arguments_count() == 5,
        "SimDevice launchApplicationWithID:options:error: has an unexpected type encoding"
    );
    let terminate = device
        .instance_method(Sel::register(c"terminateApplicationWithID:error:"))
        .context("SimDevice terminateApplicationWithID:error: is unavailable")?;
    anyhow::ensure!(
        terminate.return_type().to_bytes().first() == Some(&b'B')
            && terminate.arguments_count() == 4,
        "SimDevice terminateApplicationWithID:error: has an unexpected type encoding"
    );
    Ok(())
}

fn sim_device_class() -> Result<&'static AnyClass> {
    AnyClass::get(c"SimDevice").context("SimDevice is unavailable")
}

fn method_returns(class: &AnyClass, selector: &CStr, code: u8) -> bool {
    class
        .instance_method(Sel::register(selector))
        .and_then(|method| method.return_type().to_bytes().first().copied())
        == Some(code)
}

fn device_set() -> Result<Retained<AnyObject>> {
    super::dyld::load()?;
    let class = AnyClass::get(c"SimServiceContext").context("SimServiceContext is unavailable")?;
    let developer = NSString::from_str(&super::dyld::developer_dir()?.to_string_lossy());
    let mut error: *mut NSError = std::ptr::null_mut();
    let context: Option<Retained<AnyObject>> = unsafe {
        msg_send![
            class,
            sharedServiceContextForDeveloperDir: &*developer,
            error: &mut error
        ]
    };
    let context = context.context(ns_error(error, "SimServiceContext was not created"))?;
    let mut error: *mut NSError = std::ptr::null_mut();
    let set: Option<Retained<AnyObject>> =
        unsafe { msg_send![&*context, defaultDeviceSetWithError: &mut error] };
    set.context(ns_error(error, "SimDeviceSet was not created"))
}

fn udid_of(device: &AnyObject) -> String {
    let value: Option<Retained<AnyObject>> = unsafe { msg_send![device, UDID] };
    let Some(value) = value else {
        return String::new();
    };
    let sel = Sel::register(c"UUIDString");
    let responds: bool = unsafe { msg_send![&*value, respondsToSelector: sel] };
    if responds {
        let uuid: Option<Retained<NSString>> = unsafe { msg_send![&*value, UUIDString] };
        if let Some(uuid) = uuid {
            let text = uuid.to_string();
            if !text.is_empty() {
                return text;
            }
        }
    }
    string_value(unsafe { msg_send![&*value, description] })
}

fn state_of(device: &AnyObject) -> String {
    if method_returns_on_device(c"stateString", b'@') {
        let text = string_prop(device, |object| unsafe { msg_send![object, stateString] });
        if !text.is_empty() {
            return text;
        }
    }
    let state: u64 = unsafe { msg_send![device, state] };
    map_sim_state(state).to_owned()
}

fn available_of(device: &AnyObject) -> bool {
    if method_returns_on_device(c"available", b'B') {
        unsafe { msg_send![device, available] }
    } else {
        true
    }
}

fn method_returns_on_device(selector: &CStr, code: u8) -> bool {
    AnyClass::get(c"SimDevice").is_some_and(|class| method_returns(class, selector, code))
}

fn string_prop(
    device: &AnyObject,
    read: impl Fn(&AnyObject) -> Option<Retained<NSString>>,
) -> String {
    string_value(read(device))
}

fn string_value(value: Option<Retained<NSString>>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn map_sim_state(state: u64) -> &'static str {
    match state {
        0 => "Creating",
        1 => "Shutdown",
        2 => "Booting",
        3 => "Booted",
        4 => "Shutting Down",
        _ => "Unknown",
    }
}

fn ns_error(error: *mut NSError, fallback: &str) -> String {
    if error.is_null() {
        fallback.into()
    } else {
        unsafe { &*error }.localizedDescription().to_string()
    }
}

/// Write Unicode text to the simulator pasteboard using an in-process SimDevice
/// setter when one exists. Returns `Ok(true)` on success, `Ok(false)` when no
/// compatible setter is present so callers can fall back to `simctl pbcopy`.
///
/// `-[SimDevice setPasteboard:]` is only the `SimDevicePasteboard` property
/// setter, not a text API. Probe content setters and SimPasteboardPlus instead.
pub fn set_pasteboard_text(udid: &str, text: &str) -> Result<bool> {
    let device = match device_by_udid(udid) {
        Ok(device) => device,
        Err(_) => return Ok(false),
    };
    if try_set_pasteboard_data(&device.object, text)? {
        return Ok(true);
    }
    if try_set_pasteboard_items(&device.object, text)? {
        return Ok(true);
    }
    if let Some(pasteboard) = pasteboard_object(&device.object) {
        if try_set_pasteboard_data(&pasteboard, text)? {
            return Ok(true);
        }
        if try_set_pasteboard_items(&pasteboard, text)? {
            return Ok(true);
        }
    }
    try_sim_pasteboard_plus(&device.object, text)
}

fn instance_responds(object: &AnyObject, selector: &CStr) -> bool {
    let sel = Sel::register(selector);
    unsafe { msg_send![object, respondsToSelector: sel] }
}

fn pasteboard_object(device: &AnyObject) -> Option<Retained<AnyObject>> {
    if !instance_responds(device, c"pasteboard") {
        return None;
    }
    unsafe { msg_send![device, pasteboard] }
}

fn ns_data(bytes: &[u8]) -> Option<Retained<AnyObject>> {
    unsafe {
        msg_send![
            class!(NSData),
            dataWithBytes: bytes.as_ptr().cast::<c_void>(),
            length: bytes.len()
        ]
    }
}

fn pasteboard_items(text: &str) -> Option<Retained<AnyObject>> {
    let data = ns_data(text.as_bytes())?;
    let key = NSString::from_str("public.utf8-plain-text");
    let dict: Retained<AnyObject> =
        unsafe { msg_send![class!(NSDictionary), dictionaryWithObject: &*data, forKey: &*key] };
    unsafe { msg_send![class!(NSArray), arrayWithObject: &*dict] }
}

fn try_set_pasteboard_data(object: &AnyObject, text: &str) -> Result<bool> {
    if !instance_responds(object, c"setPasteboardWithData:error:") {
        return Ok(false);
    }
    let Some(data) = ns_data(text.as_bytes()) else {
        return Ok(false);
    };
    let mut error: *mut NSError = std::ptr::null_mut();
    let ok: bool = unsafe { msg_send![object, setPasteboardWithData: &*data, error: &mut error] };
    anyhow::ensure!(ok, ns_error(error, "setPasteboardWithData:error: failed"));
    Ok(true)
}

fn try_set_pasteboard_items(object: &AnyObject, text: &str) -> Result<bool> {
    if !instance_responds(object, c"setPasteboardWithItems:error:") {
        return Ok(false);
    }
    let Some(items) = pasteboard_items(text) else {
        return Ok(false);
    };
    let mut error: *mut NSError = std::ptr::null_mut();
    let ok: bool = unsafe { msg_send![object, setPasteboardWithItems: &*items, error: &mut error] };
    anyhow::ensure!(ok, ns_error(error, "setPasteboardWithItems:error: failed"));
    Ok(true)
}

fn try_sim_pasteboard_plus(device: &AnyObject, text: &str) -> Result<bool> {
    load_pasteboard_frameworks();
    let Some(interface_class) = AnyClass::get(c"_TtC17SimPasteboardPlus22SimPasteboardInterface")
    else {
        return Ok(false);
    };
    let Some(listener_class) =
        AnyClass::get(c"_TtC17SimPasteboardPlus30SimPasteboardInterfaceListener")
    else {
        return Ok(false);
    };
    let Some(pasteboard_class) = AnyClass::get(c"NSPasteboard") else {
        return Ok(false);
    };
    if !instance_responds(device, c"lookup:error:") {
        return Ok(false);
    }
    let service: Option<Retained<NSString>> = unsafe { msg_send![listener_class, machServiceName] };
    let Some(service) = service else {
        return Ok(false);
    };
    let mut error: *mut NSError = std::ptr::null_mut();
    let port: u32 = unsafe { msg_send![device, lookup: &*service, error: &mut error] };
    if port == 0 {
        return Ok(false);
    }
    let pasteboard: Option<Retained<AnyObject>> =
        unsafe { msg_send![pasteboard_class, pasteboardWithUniqueName] };
    let Some(pasteboard) = pasteboard else {
        return Ok(false);
    };
    let ns_text = NSString::from_str(text);
    let objects: Retained<NSArray<NSString>> = NSArray::from_retained_slice(&[ns_text]);
    let _: isize = unsafe { msg_send![&*pasteboard, clearContents] };
    let written: bool = unsafe { msg_send![&*pasteboard, writeObjects: &*objects] };
    if !written {
        return Ok(false);
    }
    let allocated: *mut AnyObject = unsafe { msg_send![interface_class, alloc] };
    if allocated.is_null() {
        return Ok(false);
    }
    let initialized: *mut AnyObject = unsafe {
        msg_send![
            allocated,
            initWithConnectingToPort: port,
            managingPasteboard: &*pasteboard,
            delegate: std::ptr::null_mut::<AnyObject>(),
            delegateQueue: std::ptr::null_mut::<AnyObject>()
        ]
    };
    let Some(interface) = (unsafe { Retained::from_raw(initialized) }) else {
        return Ok(false);
    };
    let _: () = unsafe { msg_send![&*interface, push] };
    Ok(true)
}

fn load_pasteboard_frameworks() {
    static LOADED: OnceLock<()> = OnceLock::new();
    let _ = LOADED.get_or_init(|| {
        dlopen("/System/Library/Frameworks/AppKit.framework/AppKit");
        let developer = super::dyld::developer_dir().ok();
        let mut paths = vec![PathBuf::from(
            "/Library/Developer/PrivateFrameworks/CoreSimulator.framework/Versions/A/Frameworks/SimPasteboardPlus.framework/SimPasteboardPlus",
        )];
        if let Some(developer) = developer {
            paths.push(developer.join(
                "Library/PrivateFrameworks/CoreSimulator.framework/Versions/A/Frameworks/SimPasteboardPlus.framework/SimPasteboardPlus",
            ));
            paths.push(developer.join(
                "Library/PrivateFrameworks/CoreSimulator.framework/Frameworks/SimPasteboardPlus.framework/SimPasteboardPlus",
            ));
        }
        for path in paths {
            dlopen(&path);
        }
    });
}

fn dlopen(path: impl AsRef<Path>) {
    let path = path.as_ref();
    if !path.is_file() {
        return;
    }
    let Ok(c_path) = CString::new(path.to_string_lossy().as_bytes()) else {
        return;
    };
    let _ = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_GLOBAL) };
}

pub(super) fn device_io(device: &SimDevice) -> Result<Retained<AnyObject>> {
    if instance_responds(&device.object, c"ioWithError:") {
        let mut error: *mut NSError = std::ptr::null_mut();
        let io: Option<Retained<AnyObject>> =
            unsafe { msg_send![&*device.object, ioWithError: &mut error] };
        return io.context(ns_error(error, "SimDevice.ioWithError: returned nil"));
    }
    if instance_responds(&device.object, c"io") {
        let io: Option<Retained<AnyObject>> = unsafe { msg_send![&*device.object, io] };
        return io.context("SimDevice.io returned nil");
    }
    anyhow::bail!("SimDevice has no io selector")
}

pub(super) fn framebuffer_descriptors(device: &SimDevice) -> Result<Vec<Retained<AnyObject>>> {
    let io = device_io(device)?;
    if instance_responds(&io, c"updateIOPorts") {
        let _: *mut AnyObject = unsafe { msg_send![&*io, updateIOPorts] };
    }
    let ports = io_ports(&io)?;
    let mut matched = Vec::new();
    let mut surfaces = Vec::new();
    for port in &ports {
        let Some(descriptor) = descriptor_with_surface(port) else {
            continue;
        };
        if is_framebuffer_port(port) {
            matched.push(descriptor);
        } else {
            surfaces.push(descriptor);
        }
    }
    let descriptors = if matched.is_empty() {
        surfaces
    } else {
        matched
    };
    anyhow::ensure!(
        !descriptors.is_empty(),
        "SimDevice has no framebuffer IOSurface descriptor"
    );
    Ok(descriptors)
}

pub(super) fn register_sel() -> Sel {
    Sel::register(c"registerScreenCallbacksWithUUID:callbackQueue:frameCallback:surfacesChangedCallback:propertiesChangedCallback:")
}

pub(super) fn unregister_sel() -> Sel {
    Sel::register(c"unregisterScreenCallbacksWithUUID:")
}

pub(super) fn framebuffer_surface_sel() -> Sel {
    Sel::register(c"framebufferSurface")
}

pub(super) fn sim_device_has_io() -> bool {
    let Some(class) = AnyClass::get(c"SimDevice") else {
        return false;
    };
    class.instance_method(Sel::register(c"io")).is_some()
        || class
            .instance_method(Sel::register(c"ioWithError:"))
            .is_some()
}

pub(super) fn screen_selectors_present() -> bool {
    if !sim_device_has_io() {
        return false;
    }
    let io = AnyClass::get(c"SimDeviceIOClient").or_else(|| AnyClass::get(c"SimDeviceIO"));
    let Some(io) = io else {
        return false;
    };
    let has_ports = io.instance_method(Sel::register(c"ioPorts")).is_some()
        || io
            .instance_method(Sel::register(c"deviceIOPorts"))
            .is_some()
        || io
            .instance_method(Sel::register(c"updateIOPorts"))
            .is_some();
    if !has_ports {
        return false;
    }
    let register = register_sel();
    let surface = framebuffer_surface_sel();
    for name in [
        c"_TtC12SimulatorKit15SimDeviceScreen",
        c"_TtC12SimulatorKit24SimDisplayRenderableView",
        c"SimDisplayDescriptorState",
        c"SimDisplayIOSurfaceRenderable",
        c"SimDeviceIOPortDescriptorState",
    ] {
        if let Some(class) = AnyClass::get(name) {
            if class.instance_method(register).is_some() || class.instance_method(surface).is_some()
            {
                return true;
            }
        }
    }
    true
}

fn io_ports(io: &AnyObject) -> Result<Vec<Retained<AnyObject>>> {
    if instance_responds(io, c"ioPorts") {
        let ports: Option<Retained<AnyObject>> = unsafe { msg_send![io, ioPorts] };
        if let Some(ports) = ports {
            let items = nsarray_items(&ports);
            if !items.is_empty() {
                return Ok(items);
            }
        }
    }
    if instance_responds(io, c"deviceIOPorts") {
        let ports: Option<Retained<AnyObject>> = unsafe { msg_send![io, deviceIOPorts] };
        if let Some(ports) = ports {
            let items = nsarray_items(&ports);
            if !items.is_empty() {
                return Ok(items);
            }
        }
    }
    anyhow::bail!("SimDevice io has no ioPorts")
}

fn descriptor_with_surface(port: &AnyObject) -> Option<Retained<AnyObject>> {
    let surface = framebuffer_surface_sel();
    if instance_responds(port, c"descriptor") {
        let descriptor: Option<Retained<AnyObject>> = unsafe { msg_send![port, descriptor] };
        if let Some(descriptor) = descriptor {
            let responds: bool = unsafe { msg_send![&*descriptor, respondsToSelector: surface] };
            if responds {
                return Some(descriptor);
            }
        }
    }
    let responds: bool = unsafe { msg_send![port, respondsToSelector: surface] };
    if responds { Some(port.retain()) } else { None }
}

fn is_framebuffer_port(port: &AnyObject) -> bool {
    for text in [
        named_string(port, c"portIdentifier", |object| unsafe {
            msg_send![object, portIdentifier]
        }),
        named_string(port, c"identifier", |object| unsafe {
            msg_send![object, identifier]
        }),
        named_string(port, c"uuid", |object| unsafe { msg_send![object, uuid] }),
    ]
    .into_iter()
    .flatten()
    {
        if text == "com.apple.framebuffer.display" || text.contains("framebuffer") {
            return true;
        }
    }
    false
}

fn named_string(
    port: &AnyObject,
    selector: &CStr,
    read: impl Fn(&AnyObject) -> Option<Retained<AnyObject>>,
) -> Option<String> {
    if !instance_responds(port, selector) {
        return None;
    }
    read(port).map(|value| object_string(&value))
}

fn nsarray_items(obj: &AnyObject) -> Vec<Retained<AnyObject>> {
    let is_array: bool = unsafe { msg_send![obj, isKindOfClass: class!(NSArray)] };
    if !is_array {
        return Vec::new();
    }
    let count: usize = unsafe { msg_send![obj, count] };
    let mut items = Vec::with_capacity(count);
    for index in 0..count {
        let item: *mut AnyObject = unsafe { msg_send![obj, objectAtIndex: index] };
        if let Some(item) = unsafe { Retained::retain(item) } {
            items.push(item);
        }
    }
    items
}

fn object_string(obj: &AnyObject) -> String {
    let description: Option<Retained<NSString>> = unsafe { msg_send![obj, description] };
    description
        .map(|value| value.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::map_sim_state;

    #[test]
    fn maps_core_simulator_device_states() {
        assert_eq!(map_sim_state(1), "Shutdown");
        assert_eq!(map_sim_state(3), "Booted");
        assert_eq!(map_sim_state(4), "Shutting Down");
        assert_eq!(map_sim_state(99), "Unknown");
    }
}
