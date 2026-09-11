//! Host-side AccessibilityPlatformTranslation inspect fallback.
//!
//! Used when the in-sim guest cannot read AX. The host talks to a simulator
//! through CoreSimulator (`SimDevice.sendAccessibilityRequestAsync:`) by
//! installing a token delegate on `AXPTranslator`. Simulator.app is not
//! required when that SimDevice selector is present; if a live inspect still
//! returns an empty tree, the error reports whether Simulator.app was running.

use crate::ui::{Element, Screen};
use anyhow::{Context, Result, bail};
use block2::{Block, RcBlock};
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::exception::Exception;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, NSObject, Sel};
use objc2::{ClassType, Message, class, msg_send, sel};
use objc2_foundation::NSString;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_void};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

static XPC_SENDS: AtomicUsize = AtomicUsize::new(0);
static XPC_REPLIES: AtomicUsize = AtomicUsize::new(0);
const MAX_DEPTH: usize = 48;
const MAX_ELEMENTS: usize = 4096;
const XPC_TIMEOUT: Duration = Duration::from_secs(8);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl RefEncode for CGPoint {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}
unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl RefEncode for CGSize {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}
unsafe impl Encode for CGRect {
    const ENCODING: Encoding = Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}
unsafe impl RefEncode for CGRect {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

#[derive(Clone, Debug, PartialEq)]
struct AptNode {
    role: String,
    identifier: Option<String>,
    label: Option<String>,
    value: Option<String>,
    frame: Option<[f64; 4]>,
    children: Vec<AptNode>,
}

struct TokenEntry {
    device: Retained<AnyObject>,
}

unsafe impl Send for TokenEntry {}
unsafe impl Sync for TokenEntry {}

struct SendObj(Option<Retained<AnyObject>>);

unsafe impl Send for SendObj {}

#[allow(dead_code)]
struct KeptBlock(RcBlock<dyn Fn(*mut AnyObject) -> *mut AnyObject>);

unsafe impl Send for KeptBlock {}
unsafe impl Sync for KeptBlock {}

struct DelegateGuard {
    translator: Retained<AnyObject>,
    previous: Option<Retained<AnyObject>>,
}

impl Drop for DelegateGuard {
    fn drop(&mut self) {
        let previous = self
            .previous
            .as_deref()
            .map(|object| object as *const AnyObject as *mut AnyObject)
            .unwrap_or(std::ptr::null_mut());
        let _: () = unsafe { msg_send![&*self.translator, setBridgeTokenDelegate: previous] };
    }
}

fn tokens() -> std::sync::MutexGuard<'static, HashMap<String, TokenEntry>> {
    static TOKENS: OnceLock<Mutex<HashMap<String, TokenEntry>>> = OnceLock::new();
    TOKENS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
}

fn kept_blocks() -> std::sync::MutexGuard<'static, Vec<KeptBlock>> {
    static BLOCKS: OnceLock<Mutex<Vec<KeptBlock>>> = OnceLock::new();
    BLOCKS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap()
}

pub fn available() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| std::panic::catch_unwind(probe_apt).unwrap_or(false))
}

pub fn inspect(device: &str, pid: Option<u32>) -> Result<Screen> {
    anyhow::ensure!(
        available(),
        "AccessibilityPlatformTranslation selectors are unavailable"
    );
    XPC_SENDS.store(0, Ordering::SeqCst);
    XPC_REPLIES.store(0, Ordering::SeqCst);
    super::dyld::load()?;
    let sim = super::coresim::device_by_udid(device)?;
    let translator = shared_translator()?;
    let dispatcher = dispatcher_object()?;
    match objc2::exception::catch(AssertUnwindSafe(|| {
        inspect_caught(device, pid, &sim.object, &translator, &dispatcher)
    })) {
        Ok(result) => result,
        Err(exception) => bail!("{}", exception_message(exception)),
    }
}

fn inspect_caught(
    device: &str,
    pid: Option<u32>,
    sim: &AnyObject,
    translator: &AnyObject,
    dispatcher: &AnyObject,
) -> Result<Screen> {
    if instance_responds(translator, c"enableAccessibility") && !is_abstract_translator(translator)
    {
        let _: () = unsafe { msg_send![translator, enableAccessibility] };
    }
    if instance_responds(translator, c"setAccessibilityEnabled:") {
        let _: () = unsafe { msg_send![translator, setAccessibilityEnabled: true] };
    }
    if instance_responds(translator, c"setSupportsDelegateTokens:") {
        let _: () = unsafe { msg_send![translator, setSupportsDelegateTokens: true] };
    }
    if instance_responds(translator, c"initializeAXRuntimeForSystemAppServer")
        && !is_abstract_translator(translator)
    {
        let _ = objc2::exception::catch(AssertUnwindSafe(|| {
            let _: () = unsafe { msg_send![translator, initializeAXRuntimeForSystemAppServer] };
        }));
    }

    let dispatcher_screen = {
        let previous: Option<Retained<AnyObject>> =
            unsafe { msg_send![translator, bridgeTokenDelegate] };
        let _guard = DelegateGuard {
            translator: translator.retain(),
            previous,
        };
        let _: () = unsafe { msg_send![translator, setBridgeTokenDelegate: dispatcher] };
        let token = Uuid::new_v4().to_string();
        tokens().insert(
            token.clone(),
            TokenEntry {
                device: sim.retain(),
            },
        );
        let token_ns = NSString::from_str(&token);
        let result = inspect_with_token(translator, sim, as_any(&*token_ns), device, pid);
        tokens().remove(&token);
        result
    };

    if let Ok(screen) = &dispatcher_screen {
        if useful_tree(screen) {
            return dispatcher_screen;
        }
    }

    if let Some(token) = device_apt_token(sim) {
        if let Ok(screen) = inspect_with_token(translator, sim, &token, device, pid) {
            if useful_tree(&screen) {
                return Ok(screen);
            }
        }
    }

    match dispatcher_screen {
        Ok(screen) if useful_tree(&screen) => Ok(screen),
        Ok(_) => Err(anyhow::anyhow!(apt_empty_error(
            "APT returned only a root Application without identifiers"
        ))),
        Err(error) => Err(error),
    }
}

fn useful_tree(screen: &Screen) -> bool {
    screen.elements.iter().any(|element| {
        element.identifier.is_some() || element.label.is_some() || element.value.is_some()
    })
}

fn device_apt_token(device: &AnyObject) -> Option<Retained<AnyObject>> {
    if !instance_responds(device, c"accessibilityPlatformTranslationToken") {
        return None;
    }
    unsafe { msg_send![device, accessibilityPlatformTranslationToken] }
}

fn as_any(object: &impl Message) -> &AnyObject {
    unsafe { &*(std::ptr::from_ref(object) as *const AnyObject) }
}

fn exception_message(exception: Option<Retained<Exception>>) -> String {
    let detail = match exception {
        Some(exception) => exception.to_string(),
        None => "unknown Objective-C exception".into(),
    };
    format!(
        "host APT NSException ({detail}; class=AXPTranslator_macOS selector=frontmostApplicationWithDisplayId:bridgeDelegateToken: / AXPTranslatorRequest.setRequestType: / SimDevice.sendAccessibilityRequestAsync:completionQueue:completionHandler:; simulator_app={})",
        simulator_app_running()
    )
}

/// Attribute request. Live `AXPTranslatorRequest` descriptions and
/// public idb logs encode this as `Type: 2` with `attributeType` set
/// (`AXPAttributeChildren`, role, identifier, …). Type 2 without an
/// attribute type returns a non-zero error; we never treat that as data.
const REQUEST_TYPE_ATTRIBUTE: u64 = 2;

fn inspect_with_token(
    translator: &AnyObject,
    device: &AnyObject,
    token: &AnyObject,
    udid: &str,
    pid: Option<u32>,
) -> Result<Screen> {
    let point_size = device_point_size(device);
    let mut roots = Vec::new();
    if let Some(translation) = translation_for_pid(translator, token, pid) {
        roots.push(translation);
    }
    if let Some(translation) = frontmost_application(translator, token) {
        if !roots
            .iter()
            .any(|existing| same_translation(existing, &translation))
        {
            roots.push(translation);
        }
    }
    anyhow::ensure!(
        !roots.is_empty(),
        apt_empty_error("no AXPTranslationObject")
    );

    let mut best: Option<(Vec<Element>, CGRect, Option<u32>)> = None;
    for translation in &roots {
        stamp_token(translation, token);
        let (nodes, root_frame, mut seen) =
            walk_translation(translator, token, translation, point_size);
        let mut elements = flatten_nodes(&nodes);
        if !elements
            .iter()
            .any(|element| element.identifier.is_some() || element.label.is_some())
        {
            let mut extra = nodes;
            for hit in hit_test_elements(translator, token, root_frame, point_size) {
                if let Some(hit_translation) = element_translation(&hit) {
                    stamp_token(&hit_translation, token);
                    if !seen.insert(seen_key(&hit_translation, Some(&hit))) {
                        continue;
                    }
                    extra.extend(
                        walk_translation(translator, token, &hit_translation, point_size).0,
                    );
                }
            }
            elements = flatten_nodes(&extra);
        }
        if pid.is_some_and(|expected| translation_pid(translation) != Some(expected)) {
            continue;
        }
        let resolved_pid = pid
            .or_else(|| translation_pid(translation))
            .filter(|pid| *pid > 0);
        let candidate = (elements, root_frame, resolved_pid);
        if useful_elements(&candidate.0) {
            return Ok(screen_from_walk(udid, point_size, candidate));
        }
        if best
            .as_ref()
            .is_none_or(|current| current.0.len() < candidate.0.len())
        {
            best = Some(candidate);
        }
    }
    let Some(candidate) = best else {
        bail!(apt_empty_error("attribute walk produced no nodes"));
    };
    anyhow::ensure!(
        !candidate.0.is_empty(),
        apt_empty_error("flattened APT tree was empty")
    );
    Ok(screen_from_walk(udid, point_size, candidate))
}

fn useful_elements(elements: &[Element]) -> bool {
    elements.iter().any(|element| {
        element.identifier.is_some() || element.label.is_some() || element.value.is_some()
    })
}

fn screen_from_walk(
    udid: &str,
    point_size: CGSize,
    candidate: (Vec<Element>, CGRect, Option<u32>),
) -> Screen {
    let (elements, root_frame, resolved_pid) = candidate;
    let hash = tree_hash(&elements);
    let width = if point_size.width > 0.0 {
        point_size.width
    } else {
        root_frame.size.width
    };
    let height = if point_size.height > 0.0 {
        point_size.height
    } else {
        root_frame.size.height
    };
    Screen {
        device: udid.into(),
        pid: resolved_pid,
        width: if width > 0.0 { width } else { 390.0 },
        height: if height > 0.0 { height } else { 844.0 },
        hash: Some(hash),
        elements,
    }
}

fn same_translation(left: &AnyObject, right: &AnyObject) -> bool {
    let left_key = seen_key(left, None);
    let right_key = seen_key(right, None);
    (left_key != 0 && left_key == right_key) || std::ptr::eq(left, right)
}

fn walk_translation(
    translator: &AnyObject,
    token: &AnyObject,
    translation: &AnyObject,
    point_size: CGSize,
) -> (Vec<AptNode>, CGRect, HashSet<u64>) {
    stamp_token(translation, token);
    let element = mac_platform_element(translator, translation);
    if let Some(element) = &element {
        stamp_element_token(element, token);
    }
    let types = AttrTypes::from_element(element.as_deref());
    let root_frame = element
        .as_ref()
        .and_then(|element| read_frame(element))
        .or_else(|| {
            types.frame.and_then(|attr| {
                attribute_result(token, translation, attr).and_then(|value| value_to_rect(&value))
            })
        })
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let mut nodes = Vec::new();
    collect_from_translation(
        translator,
        token,
        translation,
        element.as_deref(),
        &types,
        &root_frame,
        point_size,
        0,
        &mut seen,
        &mut nodes,
    );
    (nodes, root_frame, seen)
}

fn probe_apt() -> bool {
    let Ok(libs) = super::dyld::load() else {
        return false;
    };
    if libs.apt.is_null() {
        return false;
    }
    let Some(translator) = AnyClass::get(c"AXPTranslator") else {
        return false;
    };
    let Some(request) = AnyClass::get(c"AXPTranslatorRequest") else {
        return false;
    };
    let Some(response) = AnyClass::get(c"AXPTranslatorResponse") else {
        return false;
    };
    let Some(element) = AnyClass::get(c"AXPMacPlatformElement") else {
        return false;
    };
    let Some(device) = AnyClass::get(c"SimDevice") else {
        return false;
    };
    let has_shared = class_method_matches(translator, c"sharedInstance", b'@', 2)
        || class_method_matches(translator, c"sharediOSInstance", b'@', 2);
    has_shared
        && method_matches(
            translator,
            c"frontmostApplicationWithDisplayId:bridgeDelegateToken:",
            b'@',
            4,
        )
        && method_matches(translator, c"macPlatformElementFromTranslation:", b'@', 3)
        && method_matches(translator, c"setBridgeTokenDelegate:", b'v', 3)
        && method_matches(translator, c"enableAccessibility", b'v', 2)
        && method_matches(
            device,
            c"sendAccessibilityRequestAsync:completionQueue:completionHandler:",
            b'v',
            5,
        )
        && class_method_matches(response, c"emptyResponse", b'@', 2)
        && class_method_matches(request, c"requestWithTranslation:", b'@', 3)
        && method_matches(request, c"setRequestType:", b'v', 3)
        && method_matches(request, c"setAttributeType:", b'v', 3)
        && method_matches(response, c"resultData", b'@', 2)
        && method_matches(element, c"accessibilityRole", b'@', 2)
        && method_matches(element, c"accessibilityAttributeValue:", b'@', 3)
}

fn method_matches(class: &AnyClass, selector: &CStr, code: u8, argc: usize) -> bool {
    class
        .instance_method(Sel::register(selector))
        .is_some_and(|method| {
            method.return_type().to_bytes().first() == Some(&code)
                && method.arguments_count() == argc
        })
}

fn class_method_matches(class: &AnyClass, selector: &CStr, code: u8, argc: usize) -> bool {
    class
        .class_method(Sel::register(selector))
        .is_some_and(|method| {
            method.return_type().to_bytes().first() == Some(&code)
                && method.arguments_count() == argc
        })
}

fn shared_translator() -> Result<Retained<AnyObject>> {
    let class = AnyClass::get(c"AXPTranslator").context("AXPTranslator is unavailable")?;
    // Host-side simulator inspect uses the macOS translator: it forwards
    // attribute requests through the token delegate. The iOS translator
    // answers from local AXUIElements and cannot see in-sim PIDs.
    for name in [
        c"sharedmacOSInstance",
        c"sharediOSInstance",
        c"sharedInstance",
    ] {
        if !class_method_matches(class, name, b'@', 2) {
            continue;
        }
        let translator: Option<Retained<AnyObject>> = if name == c"sharediOSInstance" {
            unsafe { msg_send![class, sharediOSInstance] }
        } else if name == c"sharedmacOSInstance" {
            unsafe { msg_send![class, sharedmacOSInstance] }
        } else {
            unsafe { msg_send![class, sharedInstance] }
        };
        let Some(translator) = translator else {
            continue;
        };
        if is_abstract_translator(&translator) {
            continue;
        }
        return Ok(translator);
    }
    bail!("AXPTranslator concrete shared instance returned nil")
}

fn is_abstract_translator(translator: &AnyObject) -> bool {
    let class: *const AnyClass = unsafe { msg_send![translator, class] };
    let Some(class) = (unsafe { class.as_ref() }) else {
        return true;
    };
    class.name().to_bytes() == b"AXPTranslator"
}

fn dispatcher_class() -> Result<&'static AnyClass> {
    if let Some(class) = AnyClass::get(c"MxAptTokenDispatcher") {
        return Ok(class);
    }
    let mut builder = ClassBuilder::new(c"MxAptTokenDispatcher", NSObject::class())
        .context("Could not create MxAptTokenDispatcher")?;
    unsafe {
        builder.add_method(
            sel!(accessibilityTranslationDelegateBridgeCallbackWithToken:),
            bridge_callback as unsafe extern "C-unwind" fn(_, _, _) -> _,
        );
        builder.add_method(
            sel!(accessibilityTranslationConvertPlatformFrameToSystem:withToken:),
            convert_frame as unsafe extern "C-unwind" fn(_, _, _, _) -> _,
        );
        builder.add_method(
            sel!(accessibilityTranslationRootParentWithToken:),
            root_parent as unsafe extern "C-unwind" fn(_, _, _) -> _,
        );
    }
    Ok(builder.register())
}

fn dispatcher_object() -> Result<Retained<AnyObject>> {
    static OBJECT: OnceLock<usize> = OnceLock::new();
    let ptr = *OBJECT.get_or_init(|| {
        let class = match dispatcher_class() {
            Ok(class) => class,
            Err(_) => return 0,
        };
        let object: Option<Retained<AnyObject>> = unsafe { msg_send![class, new] };
        match object {
            Some(object) => Retained::into_raw(object) as usize,
            None => 0,
        }
    });
    anyhow::ensure!(ptr != 0, "MxAptTokenDispatcher was not created");
    unsafe { Retained::retain(ptr as *mut AnyObject) }.context("MxAptTokenDispatcher retain failed")
}

unsafe extern "C-unwind" fn bridge_callback(
    _this: &AnyObject,
    _cmd: Sel,
    token: *mut AnyObject,
) -> *mut AnyObject {
    let key = token_string(token);
    let block: RcBlock<dyn Fn(*mut AnyObject) -> *mut AnyObject> =
        RcBlock::new(move |request: *mut AnyObject| send_or_empty(&key, request));
    let ptr = RcBlock::as_ptr(&block) as *mut Block<dyn Fn(*mut AnyObject) -> *mut AnyObject>
        as *mut AnyObject;
    let mut kept = kept_blocks();
    if kept.len() > 64 {
        kept.remove(0);
    }
    kept.push(KeptBlock(block));
    ptr
}

unsafe extern "C-unwind" fn convert_frame(
    _this: &AnyObject,
    _cmd: Sel,
    rect: CGRect,
    _token: *mut AnyObject,
) -> CGRect {
    rect
}

unsafe extern "C-unwind" fn root_parent(
    _this: &AnyObject,
    _cmd: Sel,
    _token: *mut AnyObject,
) -> *mut AnyObject {
    std::ptr::null_mut()
}

fn send_or_empty(token: &str, request: *mut AnyObject) -> *mut AnyObject {
    let token = token.to_owned();
    match objc2::exception::catch(AssertUnwindSafe(move || {
        send_accessibility_request(&token, request)
    })) {
        Ok(Some(response)) => Retained::into_raw(response).cast(),
        _ => empty_response(),
    }
}

fn send_accessibility_request(token: &str, request: *mut AnyObject) -> Option<Retained<AnyObject>> {
    if request.is_null() {
        return None;
    }
    let device = tokens().get(token)?.device.clone();
    let request = unsafe { Retained::retain(request) }?;
    let done = std::sync::Arc::new((Mutex::new(None::<SendObj>), std::sync::Condvar::new()));
    let slot = std::sync::Arc::clone(&done);
    let block = RcBlock::new(move |response: *mut AnyObject| {
        XPC_REPLIES.fetch_add(1, Ordering::SeqCst);
        let retained = unsafe { Retained::retain(response) };
        *slot.0.lock().unwrap() = Some(SendObj(retained));
        slot.1.notify_all();
    });
    XPC_SENDS.fetch_add(1, Ordering::SeqCst);
    let queue = completion_queue();
    let _: () = unsafe {
        msg_send![
            &*device,
            sendAccessibilityRequestAsync: &*request,
            completionQueue: queue,
            completionHandler: &*block
        ]
    };
    let (mutex, cvar) = &*done;
    let mut state = mutex.lock().unwrap();
    let deadline = Instant::now() + XPC_TIMEOUT;
    while state.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let (next, timeout) = cvar.wait_timeout(state, remaining).unwrap();
        state = next;
        if timeout.timed_out() {
            return None;
        }
    }
    state.take().and_then(|response| response.0)
}

fn empty_response() -> *mut AnyObject {
    let Some(class) = AnyClass::get(c"AXPTranslatorResponse") else {
        return std::ptr::null_mut();
    };
    let response: Option<Retained<AnyObject>> = unsafe { msg_send![class, emptyResponse] };
    match response {
        Some(response) => Retained::into_raw(response).cast(),
        None => std::ptr::null_mut(),
    }
}

fn completion_queue() -> *mut AnyObject {
    const QOS_CLASS_USER_INITIATED: isize = 0x19;
    static QUEUE: OnceLock<usize> = OnceLock::new();
    let ptr = *QUEUE.get_or_init(|| {
        let serial = unsafe { dispatch_queue_create(c"mx.apt".as_ptr(), std::ptr::null()) };
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
    fn dispatch_queue_create(label: *const i8, attr: *const c_void) -> *mut AnyObject;
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut AnyObject;
}

fn translation_for_pid(
    translator: &AnyObject,
    token: &AnyObject,
    pid: Option<u32>,
) -> Option<Retained<AnyObject>> {
    let pid = pid.filter(|pid| *pid > 0)?;
    let class = AnyClass::get(c"AXPTranslator")?;
    if method_matches(class, c"translationApplicationObjectForPid:", b'@', 3) {
        let translation: Option<Retained<AnyObject>> =
            unsafe { msg_send![translator, translationApplicationObjectForPid: pid as i32] };
        if let Some(translation) = translation {
            stamp_token(&translation, token);
            return Some(translation);
        }
    }
    None
}

fn frontmost_application(translator: &AnyObject, token: &AnyObject) -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"AXPTranslator")?;
    if !method_matches(
        class,
        c"frontmostApplicationWithDisplayId:bridgeDelegateToken:",
        b'@',
        4,
    ) {
        return None;
    }
    unsafe {
        msg_send![
            translator,
            frontmostApplicationWithDisplayId: 0u32,
            bridgeDelegateToken: token
        ]
    }
}

fn mac_platform_element(
    translator: &AnyObject,
    translation: &AnyObject,
) -> Option<Retained<AnyObject>> {
    unsafe { msg_send![translator, macPlatformElementFromTranslation: translation] }
}

fn stamp_token(translation: &AnyObject, token: &AnyObject) {
    if !is_kind(translation, "AXPTranslationObject") {
        return;
    }
    if instance_responds(translation, c"setBridgeDelegateToken:") {
        let _: () = unsafe { msg_send![translation, setBridgeDelegateToken: token] };
    }
    if instance_responds(translation, c"setValue:forKey:") {
        let key = NSString::from_str("bridgeDelegateToken");
        let _: () = unsafe { msg_send![translation, setValue: token, forKey: &*key] };
    }
}

fn stamp_element_token(element: &AnyObject, token: &AnyObject) {
    if !instance_responds(element, c"translation") {
        return;
    }
    let translation: Option<Retained<AnyObject>> = unsafe { msg_send![element, translation] };
    if let Some(translation) = translation {
        stamp_token(&translation, token);
    }
}

fn collect_from_translation(
    translator: &AnyObject,
    token: &AnyObject,
    translation: &AnyObject,
    element: Option<&AnyObject>,
    types: &AttrTypes,
    root_frame: &CGRect,
    point_size: CGSize,
    depth: usize,
    seen: &mut HashSet<u64>,
    out: &mut Vec<AptNode>,
) {
    if depth > MAX_DEPTH || out.len() >= MAX_ELEMENTS {
        return;
    }
    let Some((translation, element)) =
        as_translation_and_element(translator, token, translation, element)
    else {
        return;
    };
    stamp_token(&translation, token);
    if !seen.insert(seen_key(&translation, element.as_deref())) {
        return;
    }
    let role = normalize_role(
        types
            .role
            .and_then(|attr| attribute_string_result(token, &translation, attr))
            .or_else(|| {
                element
                    .as_ref()
                    .and_then(|element| read_string(element, "accessibilityRole"))
            })
            .or_else(|| {
                element
                    .as_ref()
                    .and_then(|element| read_string(element, "role"))
            })
            .or_else(|| {
                element
                    .as_ref()
                    .and_then(|element| attribute_string(element, "AXRole"))
            }),
    );
    let identifier = types
        .identifier
        .and_then(|attr| attribute_string_result(token, &translation, attr))
        .or_else(|| {
            element
                .as_ref()
                .and_then(|element| attribute_string(element, "AXIdentifier"))
        })
        .or_else(|| {
            element
                .as_ref()
                .and_then(|element| attribute_string(element, "AXUniqueId"))
        });
    let label = ["AXLabel", "AXTitle", "AXDescription"]
        .iter()
        .find_map(|name| {
            mac_attr_type(element.as_deref(), name)
                .filter(|attr| *attr != 0)
                .and_then(|attr| attribute_string_result(token, &translation, attr))
        })
        .or_else(|| {
            element
                .as_ref()
                .and_then(|element| read_string(element, "accessibilityLabel"))
        })
        .or_else(|| {
            element
                .as_ref()
                .and_then(|element| attribute_string(element, "AXLabel"))
        });
    let value = types
        .value
        .and_then(|attr| attribute_string_result(token, &translation, attr))
        .or_else(|| {
            element
                .as_ref()
                .and_then(|element| attribute_string(element, "AXValue"))
        });
    let frame = element
        .as_ref()
        .and_then(|element| read_frame(element))
        .or_else(|| {
            types.frame.and_then(|attr| {
                attribute_result(token, &translation, attr).and_then(|value| value_to_rect(&value))
            })
        })
        .map(|frame| project_frame(frame, *root_frame, point_size));
    let children = child_translations(translator, token, &translation, element.as_deref(), types);
    let mut child_nodes = Vec::new();
    for child in children {
        collect_from_translation(
            translator,
            token,
            &child,
            None,
            types,
            root_frame,
            point_size,
            depth + 1,
            seen,
            &mut child_nodes,
        );
    }
    out.push(AptNode {
        role,
        identifier,
        label,
        value,
        frame,
        children: child_nodes,
    });
}

struct AttrTypes {
    children: Option<u64>,
    role: Option<u64>,
    identifier: Option<u64>,
    frame: Option<u64>,
    value: Option<u64>,
}

impl AttrTypes {
    fn from_element(element: Option<&AnyObject>) -> Self {
        Self {
            children: mac_attr_type(element, "AXChildren").or(Some(8)),
            role: mac_attr_type(element, "AXRole").or(Some(45)),
            identifier: mac_attr_type(element, "AXIdentifier").or(Some(25)),
            frame: mac_attr_type(element, "AXFrame").or(Some(21)),
            value: mac_attr_type(element, "AXValue").or(Some(53)),
        }
    }
}

fn mac_attr_type(element: Option<&AnyObject>, name: &str) -> Option<u64> {
    let element = element?;
    if !instance_responds(element, c"_attributeTypeForMacAttribute:") {
        return None;
    }
    let key = NSString::from_str(name);
    let ty: u64 = unsafe { msg_send![element, _attributeTypeForMacAttribute: &*key] };
    (ty != 0).then_some(ty)
}

fn attribute_string_result(
    token: &AnyObject,
    translation: &AnyObject,
    attr: u64,
) -> Option<String> {
    attribute_result(token, translation, attr).and_then(|value| ax_string(&value))
}

fn ax_string(value: &AnyObject) -> Option<String> {
    if is_kind(value, "NSNumber") {
        return None;
    }
    object_to_string(value)
}

fn attribute_result(
    token: &AnyObject,
    translation: &AnyObject,
    attr: u64,
) -> Option<Retained<AnyObject>> {
    let request = make_attribute_request(translation, attr)?;
    let token_key = token_string(token as *const AnyObject as *mut AnyObject);
    let response =
        send_accessibility_request(&token_key, Retained::as_ptr(&request) as *mut AnyObject)?;
    response_result(&response)
}

fn make_attribute_request(translation: &AnyObject, attr: u64) -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"AXPTranslatorRequest")?;
    let request: Option<Retained<AnyObject>> =
        unsafe { msg_send![class, requestWithTranslation: translation] };
    let request = request?;
    if instance_responds(&request, c"setRequestType:") {
        let _: () = unsafe { msg_send![&*request, setRequestType: REQUEST_TYPE_ATTRIBUTE] };
    }
    if instance_responds(&request, c"setAttributeType:") {
        let _: () = unsafe { msg_send![&*request, setAttributeType: attr] };
    }
    Some(request)
}

fn response_result(response: &AnyObject) -> Option<Retained<AnyObject>> {
    if instance_responds(response, c"error") {
        let error: u64 = unsafe { msg_send![response, error] };
        if error != 0 {
            return None;
        }
    }
    if instance_responds(response, c"resultData") {
        let data: Option<Retained<AnyObject>> = unsafe { msg_send![response, resultData] };
        if data.is_some() {
            return data;
        }
    }
    if instance_responds(response, c"translationsResponse") {
        let data: Option<Retained<AnyObject>> =
            unsafe { msg_send![response, translationsResponse] };
        if data.is_some() {
            return data;
        }
    }
    if instance_responds(response, c"translationResponse") {
        return unsafe { msg_send![response, translationResponse] };
    }
    None
}

fn child_translations(
    translator: &AnyObject,
    token: &AnyObject,
    translation: &AnyObject,
    element: Option<&AnyObject>,
    types: &AttrTypes,
) -> Vec<Retained<AnyObject>> {
    let mut children = Vec::new();
    if let Some(attr) = types.children {
        if let Some(value) = attribute_result(token, translation, attr) {
            append_child_value(translator, token, Some(value), &mut children);
        }
    }
    if let Some(element) = element {
        append_child_value(
            translator,
            token,
            attribute_value(element, "AXChildren"),
            &mut children,
        );
        append_child_value(
            translator,
            token,
            process_attribute(element, "AXChildren"),
            &mut children,
        );
        if instance_responds(element, c"valueForKey:") {
            let key = NSString::from_str("accessibilityChildren");
            let value: Option<Retained<AnyObject>> =
                unsafe { msg_send![element, valueForKey: &*key] };
            append_child_value(translator, token, value, &mut children);
        }
        for name in ["AXWindows", "AXContents", "AXMainWindow"] {
            append_child_value(
                translator,
                token,
                attribute_value(element, name),
                &mut children,
            );
        }
        for name in child_attribute_names(element) {
            append_child_value(
                translator,
                token,
                attribute_value(element, &name),
                &mut children,
            );
        }
    }
    children
}

fn as_translation_and_element(
    translator: &AnyObject,
    token: &AnyObject,
    object: &AnyObject,
    element: Option<&AnyObject>,
) -> Option<(Retained<AnyObject>, Option<Retained<AnyObject>>)> {
    if is_kind(object, "AXPTranslationObject") {
        stamp_token(object, token);
        let element = element
            .map(|element| element.retain())
            .or_else(|| mac_platform_element(translator, object));
        if let Some(element) = &element {
            stamp_element_token(element, token);
        }
        return Some((object.retain(), element));
    }
    if is_kind(object, "AXPMacPlatformElement") {
        stamp_element_token(object, token);
        let translation = element_translation(object)?;
        stamp_token(&translation, token);
        return Some((translation, Some(object.retain())));
    }
    None
}

fn seen_key(translation: &AnyObject, element: Option<&AnyObject>) -> u64 {
    if is_kind(translation, "AXPTranslationObject") {
        let id: u64 = unsafe { msg_send![translation, objectID] };
        if id != 0 {
            return id;
        }
    }
    if let Some(element) = element {
        return element as *const AnyObject as u64;
    }
    translation as *const AnyObject as u64
}

fn element_translation(element: &AnyObject) -> Option<Retained<AnyObject>> {
    if !instance_responds(element, c"translation") {
        return None;
    }
    unsafe { msg_send![element, translation] }
}

fn value_to_rect(value: &AnyObject) -> Option<CGRect> {
    if instance_responds(value, c"rectValue") {
        let frame: CGRect = unsafe { msg_send![value, rectValue] };
        return Some(frame);
    }
    if instance_responds(value, c"CGRectValue") {
        let frame: CGRect = unsafe { msg_send![value, CGRectValue] };
        return Some(frame);
    }
    None
}

fn hit_test_elements(
    translator: &AnyObject,
    token: &AnyObject,
    root_frame: CGRect,
    point_size: CGSize,
) -> Vec<Retained<AnyObject>> {
    if !instance_responds(translator, c"objectAtPoint:displayId:bridgeDelegateToken:") {
        return Vec::new();
    }
    let mut hits = Vec::new();
    if root_frame.size.width > 1.0 && root_frame.size.height > 1.0 {
        hits.extend(hit_test_grid(
            translator,
            token,
            root_frame.origin.x,
            root_frame.origin.y,
            root_frame.size.width,
            root_frame.size.height,
        ));
    }
    if !hits_have_semantics(&hits) {
        let width = if point_size.width > 0.0 {
            point_size.width
        } else {
            390.0
        };
        let height = if point_size.height > 0.0 {
            point_size.height
        } else {
            844.0
        };
        let mut y = 32.0;
        while y < height {
            let mut x = 32.0;
            while x < width {
                let host = unmap_point(x, y, root_frame, point_size);
                if let Some(element) = hit_element_at_host(translator, token, host.x, host.y) {
                    hits.push(element);
                }
                x += 64.0;
            }
            y += 64.0;
        }
    }
    hits
}

fn unmap_point(x: f64, y: f64, root: CGRect, device: CGSize) -> CGPoint {
    if device.width.abs() < f64::EPSILON || device.height.abs() < f64::EPSILON {
        return CGPoint {
            x: root.origin.x + x,
            y: root.origin.y + y,
        };
    }
    CGPoint {
        x: root.origin.x + x * (root.size.width / device.width),
        y: root.origin.y + y * (root.size.height / device.height),
    }
}

fn hits_have_semantics(hits: &[Retained<AnyObject>]) -> bool {
    hits.iter().any(|element| {
        attribute_string(element, "AXIdentifier").is_some()
            || read_string(element, "accessibilityLabel").is_some()
            || attribute_string(element, "AXLabel").is_some()
    })
}

fn hit_test_grid(
    translator: &AnyObject,
    token: &AnyObject,
    origin_x: f64,
    origin_y: f64,
    width: f64,
    height: f64,
) -> Vec<Retained<AnyObject>> {
    let step = 64.0;
    let mut hits = Vec::new();
    let mut y = origin_y + step / 2.0;
    while y < origin_y + height {
        let mut x = origin_x + step / 2.0;
        while x < origin_x + width {
            if let Some(element) = hit_element_at_host(translator, token, x, y) {
                hits.push(element);
            }
            x += step;
        }
        y += step;
    }
    hits
}

fn hit_element_at_host(
    translator: &AnyObject,
    token: &AnyObject,
    x: f64,
    y: f64,
) -> Option<Retained<AnyObject>> {
    let point = CGPoint { x, y };
    let translation = objc2::exception::catch(AssertUnwindSafe(|| {
        let value: Option<Retained<AnyObject>> = unsafe {
            msg_send![
                translator,
                objectAtPoint: point,
                displayId: 0u32,
                bridgeDelegateToken: token
            ]
        };
        value
    }))
    .ok()
    .flatten()?;
    stamp_token(&translation, token);
    let element = mac_platform_element(translator, &translation)?;
    stamp_element_token(&element, token);
    Some(element)
}

fn child_attribute_names(element: &AnyObject) -> Vec<String> {
    if !instance_responds(element, c"accessibilityAttributeNames") {
        return Vec::new();
    }
    let names: Option<Retained<AnyObject>> =
        unsafe { msg_send![element, accessibilityAttributeNames] };
    let Some(names) = names else {
        return Vec::new();
    };
    nsarray_items(&names)
        .iter()
        .filter_map(|item| object_to_string(item))
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            !lower.contains("visible")
                && !lower.contains("interactable")
                && !lower.contains("occluded")
                && (lower.contains("child")
                    || lower.contains("window")
                    || lower.contains("content")
                    || lower.contains("elements"))
        })
        .collect()
}

fn append_child_value(
    translator: &AnyObject,
    token: &AnyObject,
    value: Option<Retained<AnyObject>>,
    elements: &mut Vec<Retained<AnyObject>>,
) {
    let Some(value) = value else {
        return;
    };
    let mut items = nsarray_items(&value);
    if items.is_empty() {
        items.push(value);
    }
    for item in items {
        let Some(platform) = wrap_platform(translator, token, item) else {
            continue;
        };
        let ptr = &*platform as *const AnyObject as usize;
        if elements
            .iter()
            .any(|existing| &**existing as *const AnyObject as usize == ptr)
        {
            continue;
        }
        elements.push(platform);
    }
}

fn wrap_platform(
    translator: &AnyObject,
    token: &AnyObject,
    item: Retained<AnyObject>,
) -> Option<Retained<AnyObject>> {
    if is_kind(&item, "AXPMacPlatformElement") {
        stamp_element_token(&item, token);
        return Some(item);
    }
    if is_kind(&item, "AXPTranslationObject") || instance_responds(&item, c"objectID") {
        stamp_token(&item, token);
        let platform = mac_platform_element(translator, &item)?;
        stamp_element_token(&platform, token);
        return Some(platform);
    }
    None
}

fn flatten_nodes(nodes: &[AptNode]) -> Vec<Element> {
    let mut elements = Vec::new();
    fn visit(nodes: &[AptNode], elements: &mut Vec<Element>) {
        for node in nodes {
            elements.push(Element {
                role: node.role.clone(),
                reference: None,
                identifier: node.identifier.clone(),
                label: node.label.clone(),
                value: node.value.clone(),
                index: Some(elements.len() as u32),
                frame: node.frame,
            });
            visit(&node.children, elements);
        }
    }
    visit(nodes, &mut elements);
    elements
}

fn tree_hash(elements: &[Element]) -> String {
    let mut hash = 5381_u64;
    for element in elements {
        hash = hash_mix(hash, Some(element.role.as_str()));
        hash = hash_mix(hash, element.identifier.as_deref());
        hash = hash_mix(hash, element.label.as_deref());
        hash = hash_mix(hash, element.value.as_deref());
    }
    format!("{hash:016x}")
}

fn hash_mix(mut hash: u64, value: Option<&str>) -> u64 {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        for byte in value.as_bytes() {
            hash = hash
                .wrapping_shl(5)
                .wrapping_add(hash)
                .wrapping_add(*byte as u64);
        }
    }
    hash.wrapping_shl(5).wrapping_add(hash).wrapping_add(0x1f)
}

fn normalize_role(role: Option<String>) -> String {
    let role = role
        .filter(|role| !role.is_empty())
        .unwrap_or_else(|| "Unknown".into());
    if let Some(rest) = role.strip_prefix("AX") {
        if !rest.is_empty()
            && rest
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
        {
            return rest.to_owned();
        }
    }
    role
}

fn project_frame(frame: CGRect, root: CGRect, device: CGSize) -> [f64; 4] {
    if root.size.width.abs() < f64::EPSILON || root.size.height.abs() < f64::EPSILON {
        return [
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
        ];
    }
    let scale_x = if device.width > 0.0 {
        device.width / root.size.width
    } else {
        1.0
    };
    let scale_y = if device.height > 0.0 {
        device.height / root.size.height
    } else {
        1.0
    };
    [
        (frame.origin.x - root.origin.x) * scale_x,
        (frame.origin.y - root.origin.y) * scale_y,
        frame.size.width * scale_x,
        frame.size.height * scale_y,
    ]
}

fn device_point_size(device: &AnyObject) -> CGSize {
    let fallback = CGSize {
        width: 393.0,
        height: 852.0,
    };
    if !instance_responds(device, c"deviceType") {
        return fallback;
    }
    let device_type: Option<Retained<AnyObject>> = unsafe { msg_send![device, deviceType] };
    let Some(device_type) = device_type else {
        return fallback;
    };
    let pixel: CGSize = if instance_responds(&device_type, c"mainScreenSize") {
        unsafe { msg_send![&*device_type, mainScreenSize] }
    } else {
        return fallback;
    };
    let scale = if instance_responds(&device_type, c"mainScreenScale") {
        let scale: f32 = unsafe { msg_send![&*device_type, mainScreenScale] };
        scale as f64
    } else {
        3.0
    };
    if scale <= 0.0 || pixel.width <= 0.0 || pixel.height <= 0.0 {
        return fallback;
    }
    CGSize {
        width: pixel.width / scale,
        height: pixel.height / scale,
    }
}

fn read_frame(element: &AnyObject) -> Option<CGRect> {
    if instance_responds(element, c"accessibilityFrame") {
        let frame: CGRect = unsafe { msg_send![element, accessibilityFrame] };
        if frame.size.width != 0.0
            || frame.size.height != 0.0
            || frame.origin.x != 0.0
            || frame.origin.y != 0.0
        {
            return Some(frame);
        }
    }
    None
}

fn read_string(element: &AnyObject, selector: &str) -> Option<String> {
    let c_sel = std::ffi::CString::new(selector).ok()?;
    if !instance_responds(element, &c_sel) {
        return None;
    }
    let sel = Sel::register(c_sel.as_c_str());
    let value: Option<Retained<AnyObject>> = unsafe { msg_send![element, performSelector: sel] };
    value.and_then(|value| object_to_string(&value))
}

fn attribute_string(element: &AnyObject, name: &str) -> Option<String> {
    attribute_value(element, name).and_then(|value| object_to_string(&value))
}

fn process_attribute(element: &AnyObject, name: &str) -> Option<Retained<AnyObject>> {
    if !instance_responds(element, c"_accessibilityProcessAttribute:") {
        return None;
    }
    let key = NSString::from_str(name);
    objc2::exception::catch(AssertUnwindSafe(|| unsafe {
        msg_send![element, _accessibilityProcessAttribute: &*key]
    }))
    .ok()
    .flatten()
}

fn attribute_value(element: &AnyObject, name: &str) -> Option<Retained<AnyObject>> {
    if !instance_responds(element, c"accessibilityAttributeValue:") {
        return None;
    }
    let key = NSString::from_str(name);
    unsafe { msg_send![element, accessibilityAttributeValue: &*key] }
}

fn translation_pid(translation: &AnyObject) -> Option<u32> {
    if !instance_responds(translation, c"pid") {
        return None;
    }
    let pid: i32 = unsafe { msg_send![translation, pid] };
    u32::try_from(pid).ok().filter(|pid| *pid > 0)
}

fn instance_responds(object: &AnyObject, selector: &CStr) -> bool {
    let sel = Sel::register(selector);
    unsafe { msg_send![object, respondsToSelector: sel] }
}

fn is_kind(object: &AnyObject, class_name: &str) -> bool {
    let Some(c_name) = std::ffi::CString::new(class_name).ok() else {
        return false;
    };
    let Some(class) = AnyClass::get(c_name.as_c_str()) else {
        return false;
    };
    unsafe { msg_send![object, isKindOfClass: class] }
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

fn object_to_string(obj: &AnyObject) -> Option<String> {
    let is_string: bool = unsafe { msg_send![obj, isKindOfClass: class!(NSString)] };
    if is_string {
        let value: Option<Retained<NSString>> = unsafe { msg_send![obj, description] };
        let text = value?.to_string();
        return (!text.is_empty()).then_some(text);
    }
    if instance_responds(obj, c"stringValue") {
        let value: Option<Retained<NSString>> = unsafe { msg_send![obj, stringValue] };
        if let Some(text) = value.map(|value| value.to_string()) {
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn token_string(token: *mut AnyObject) -> String {
    let Some(obj) = (unsafe { token.as_ref() }) else {
        return String::new();
    };
    if instance_responds(obj, c"UUIDString") {
        let uuid: Option<Retained<NSString>> = unsafe { msg_send![obj, UUIDString] };
        if let Some(uuid) = uuid {
            let text = uuid.to_string();
            if !text.is_empty() {
                return text;
            }
        }
    }
    object_to_string(obj).unwrap_or_else(|| {
        let description: Option<Retained<NSString>> = unsafe { msg_send![obj, description] };
        description
            .map(|value| value.to_string())
            .unwrap_or_default()
    })
}

fn simulator_app_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-x", "Simulator"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn apt_empty_error(detail: &str) -> String {
    format!(
        "host APT empty tree ({detail}; class=AXPTranslator_macOS selector=frontmostApplicationWithDisplayId:bridgeDelegateToken: / AXPTranslatorRequest requestType=2 attributeType=_attributeTypeForMacAttribute: / objectAtPoint:displayId:bridgeDelegateToken: / macPlatformElementFromTranslation: / SimDevice.sendAccessibilityRequestAsync:completionQueue:completionHandler:; xpc_sends={} xpc_replies={}; simulator_app={})",
        XPC_SENDS.load(Ordering::SeqCst),
        XPC_REPLIES.load(Ordering::SeqCst),
        simulator_app_running()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_does_not_panic_without_a_simulator() {
        let _ = available();
    }

    #[test]
    fn flattens_and_hashes_role_id_label_value_without_frames() {
        let tree = [AptNode {
            role: "AXButton".into(),
            identifier: Some("increment".into()),
            label: Some("Increment".into()),
            value: None,
            frame: Some([10.0, 20.0, 30.0, 40.0]),
            children: vec![AptNode {
                role: "StaticText".into(),
                identifier: Some("counter".into()),
                label: Some("0".into()),
                value: Some("0".into()),
                frame: Some([1.0, 2.0, 3.0, 4.0]),
                children: Vec::new(),
            }],
        }];
        let elements = flatten_nodes(&tree);
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].role, "AXButton");
        assert_eq!(elements[0].identifier.as_deref(), Some("increment"));
        assert_eq!(elements[0].frame, Some([10.0, 20.0, 30.0, 40.0]));
        assert_eq!(elements[1].identifier.as_deref(), Some("counter"));
        let hashed = tree_hash(&elements);
        let without_frames = flatten_nodes(&[AptNode {
            role: "AXButton".into(),
            identifier: Some("increment".into()),
            label: Some("Increment".into()),
            value: None,
            frame: None,
            children: vec![AptNode {
                role: "StaticText".into(),
                identifier: Some("counter".into()),
                label: Some("0".into()),
                value: Some("0".into()),
                frame: None,
                children: Vec::new(),
            }],
        }]);
        assert_eq!(hashed, tree_hash(&without_frames));
        assert_eq!(hashed.len(), 16);
        assert!(hashed.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_eq!(normalize_role(Some("AXButton".into())), "Button");
        assert_eq!(hash_mix(5381, None), hash_mix(5381, Some("")));
    }
}
