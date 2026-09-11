use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

struct Connection {
    stream: UnixStream,
    next_id: u64,
    #[allow(dead_code)]
    child: Option<Child>,
}

static CONNECTIONS: Mutex<Option<HashMap<String, Connection>>> = Mutex::new(None);

fn connections() -> std::sync::MutexGuard<'static, Option<HashMap<String, Connection>>> {
    CONNECTIONS.lock().unwrap()
}

fn socket_path(udid: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/mx-{udid}.sock"))
}

fn guest_binary() -> Result<PathBuf> {
    let path = super::dyld::guest_install_path();
    anyhow::ensure!(
        path.is_file(),
        "mx-guest is not installed at {}. Run sh scripts/setup-native.sh from the Mx checkout or set MX_GUEST_PATH",
        path.display()
    );
    Ok(path)
}

fn spawn_guest(udid: &str) -> Result<Child> {
    let binary = guest_binary()?;
    let socket = socket_path(udid);
    let mut command = Command::new("xcrun");
    command
        .args(["simctl", "spawn", udid])
        .arg(&binary)
        .arg("serve")
        .arg("--socket")
        .arg(&socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command.spawn().context("Could not spawn mx-guest")
}

fn connect(udid: &str) -> Result<()> {
    let path = socket_path(udid);
    if path.exists() {
        if let Ok(stream) = UnixStream::connect(&path) {
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            stream.set_write_timeout(Some(Duration::from_secs(10)))?;
            connections().get_or_insert_with(HashMap::new).insert(
                udid.into(),
                Connection {
                    stream,
                    next_id: 1,
                    child: None,
                },
            );
            try_request(udid, json!({"method": "hello"}))
                .context("mx-guest hello failed on existing socket")?;
            return Ok(());
        }
    }
    let mut child = spawn_guest(udid)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            anyhow::bail!(
                "mx-guest exited ({status}): {}",
                if stderr.trim().is_empty() {
                    "no stderr"
                } else {
                    stderr.trim()
                }
            );
        }
        if let Ok(stream) = UnixStream::connect(&path) {
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            stream.set_write_timeout(Some(Duration::from_secs(10)))?;
            connections().get_or_insert_with(HashMap::new).insert(
                udid.into(),
                Connection {
                    stream,
                    next_id: 1,
                    child: Some(child),
                },
            );
            try_request(udid, json!({"method": "hello"}))
                .context("mx-guest hello failed after spawn")?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "mx-guest did not create {} within 8 seconds",
                path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn try_request(udid: &str, mut body: Value) -> Result<Value> {
    let mut map = connections();
    let connection = map
        .as_mut()
        .and_then(|map| map.get_mut(udid))
        .context("Guest connection missing")?;
    let id = connection.next_id;
    connection.next_id += 1;
    body["id"] = json!(id);
    let encoded = super::encode::encode_frame(serde_json::to_vec(&body)?.as_slice())?;
    connection.stream.write_all(&encoded)?;
    connection.stream.flush()?;
    let reply = super::encode::read_frame(&mut connection.stream)?;
    let reply: Value = serde_json::from_slice(&reply).context("Guest returned invalid JSON")?;
    if reply["ok"] != true {
        anyhow::bail!(
            "{}",
            reply["error"]
                .as_str()
                .unwrap_or("mx-guest returned an error")
        );
    }
    Ok(reply)
}

fn request(udid: &str, body: Value) -> Result<Value> {
    for attempt in 0..2 {
        if connections()
            .as_ref()
            .is_none_or(|map| !map.contains_key(udid))
        {
            connect(udid)?;
        }
        match try_request(udid, body.clone()) {
            Ok(reply) => return Ok(reply),
            Err(error) if attempt == 0 => {
                connections().as_mut().map(|map| map.remove(udid));
                let _ = std::fs::remove_file(socket_path(udid));
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
    anyhow::bail!("mx-guest request failed")
}

fn frame_array(node: &Value) -> Option<[f64; 4]> {
    let frame = node.get("frame")?.as_array()?;
    if frame.len() != 4 {
        return None;
    }
    Some([
        frame[0].as_f64()?,
        frame[1].as_f64()?,
        frame[2].as_f64()?,
        frame[3].as_f64()?,
    ])
}

fn snapshot_request(pid: Option<u32>, want_tree: bool, if_hash_not: Option<&str>) -> Value {
    let mut body = json!({"method": "snapshot", "want_tree": want_tree});
    if let Some(pid) = pid {
        body["pid"] = json!(pid);
    }
    if let Some(hash) = if_hash_not.filter(|hash| !hash.is_empty()) {
        body["if_hash_not"] = json!(hash);
    }
    body
}

fn snapshot_unchanged(reply: &Value) -> bool {
    if reply["unchanged"].as_bool() == Some(true) {
        return true;
    }
    let hash = reply
        .get("hash")
        .and_then(Value::as_str)
        .filter(|hash| !hash.is_empty());
    if hash.is_none() {
        return false;
    }
    matches!(reply.get("elements"), None | Some(Value::Null))
}

fn snapshot_elements(reply: &Value) -> Result<Vec<crate::ui::Element>> {
    Ok(reply["elements"]
        .as_array()
        .context("Guest snapshot omitted elements")?
        .iter()
        .enumerate()
        .map(|(index, node)| crate::ui::Element {
            role: node["role"].as_str().unwrap_or("Unknown").to_owned(),
            reference: None,
            identifier: node["identifier"].as_str().map(str::to_owned),
            label: node["label"].as_str().map(str::to_owned),
            value: node["value"].as_str().map(str::to_owned),
            index: Some(index as u32),
            frame: frame_array(node),
        })
        .collect())
}

fn decode_snapshot(device: &str, reply: &Value) -> Result<crate::ui::Screen> {
    let hash = reply
        .get("hash")
        .and_then(Value::as_str)
        .filter(|hash| !hash.is_empty())
        .map(str::to_owned);
    let unchanged = snapshot_unchanged(reply);
    let elements = if unchanged {
        Vec::new()
    } else {
        snapshot_elements(reply)?
    };
    Ok(crate::ui::Screen {
        device: device.into(),
        pid: reply["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 0),
        width: reply["width"].as_f64().unwrap_or(0.0),
        height: reply["height"].as_f64().unwrap_or(0.0),
        hash,
        elements,
    })
}

pub struct Inspected {
    pub screen: crate::ui::Screen,
    pub unchanged: bool,
}

#[allow(dead_code)]
pub fn inspect(
    device: &str,
    pid: Option<u32>,
    if_hash_not: Option<&str>,
) -> Result<crate::ui::Screen> {
    Ok(inspect_detailed(device, pid, if_hash_not)?.screen)
}

pub fn inspect_detailed(
    device: &str,
    pid: Option<u32>,
    if_hash_not: Option<&str>,
) -> Result<Inspected> {
    let reply = request(device, snapshot_request(pid, true, if_hash_not))?;
    Ok(Inspected {
        screen: decode_snapshot(device, &reply)?,
        unchanged: snapshot_unchanged(&reply),
    })
}

pub fn empty_tree_error(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_lowercase();
    text.contains("empty xct") || text.contains("empty ax") || text.contains("empty tree")
}

pub fn inspect_hash(device: &str, pid: Option<u32>) -> Result<Option<String>> {
    let reply = request(device, snapshot_request(pid, false, None))?;
    Ok(reply["hash"]
        .as_str()
        .filter(|hash| !hash.is_empty())
        .map(str::to_owned))
}

pub fn press(device: &str, pid: Option<u32>, selector: &crate::ui::Selector) -> Result<()> {
    let mut body = json!({"method": "press"});
    if let Some(pid) = pid {
        body["pid"] = json!(pid);
    }
    if let Some(id) = &selector.identifier {
        body["identifier"] = json!(id);
    }
    if let Some(label) = &selector.label {
        body["label"] = json!(label);
    }
    if let Some(role) = &selector.role {
        body["role"] = json!(role);
    }
    request(device, body)?;
    Ok(())
}

#[allow(dead_code)]
pub fn set_value(device: &str, text: &str) -> Result<()> {
    request(device, json!({"method": "set_value", "text": text}))?;
    Ok(())
}

#[allow(dead_code)]
pub fn hello(device: &str) -> Result<Value> {
    request(device, json!({"method": "hello"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_elements_with_hash_is_unchanged() {
        let reply = json!({
            "ok": true,
            "pid": 4321,
            "width": 390.0,
            "height": 844.0,
            "hash": "abc123",
            "truncated": false
        });
        assert!(snapshot_unchanged(&reply));
        let screen = decode_snapshot("test-device", &reply).unwrap();
        assert_eq!(screen.hash.as_deref(), Some("abc123"));
        assert!(screen.elements.is_empty());
        assert_eq!(screen.pid, Some(4321));
        assert_eq!(screen.width, 390.0);
        assert_eq!(screen.height, 844.0);
    }

    #[test]
    fn empty_elements_with_unchanged_flag_is_unchanged() {
        let reply = json!({
            "ok": true,
            "pid": 1,
            "width": 390.0,
            "height": 844.0,
            "hash": "same",
            "truncated": false,
            "unchanged": true,
            "elements": []
        });
        assert!(snapshot_unchanged(&reply));
        let screen = decode_snapshot("d", &reply).unwrap();
        assert!(screen.elements.is_empty());
        assert_eq!(screen.hash.as_deref(), Some("same"));
    }

    #[test]
    fn version_one_reply_with_elements_is_a_full_tree() {
        let reply = json!({
            "ok": true,
            "pid": 99,
            "width": 390.0,
            "height": 844.0,
            "hash": "tree",
            "elements": [{
                "role": "Button",
                "identifier": "first",
                "label": "Continue"
            }]
        });
        assert!(!snapshot_unchanged(&reply));
        let screen = decode_snapshot("d", &reply).unwrap();
        assert_eq!(screen.elements.len(), 1);
        assert_eq!(screen.elements[0].identifier.as_deref(), Some("first"));
        assert_eq!(screen.hash.as_deref(), Some("tree"));
    }

    #[test]
    fn empty_tree_error_matches_guest_ax_copy() {
        let error =
            anyhow::anyhow!("empty XCT tree (xct=loaded-remote; timeout; empty AX tree (pid=1))");
        assert!(empty_tree_error(&error));
        assert!(!empty_tree_error(&anyhow::anyhow!("mx-guest hello failed")));
    }

    #[test]
    fn empty_tree_without_unchanged_flag_is_not_a_cache_hit() {
        let reply = json!({
            "ok": true,
            "pid": 1,
            "width": 390.0,
            "height": 844.0,
            "hash": "empty",
            "elements": []
        });
        assert!(!snapshot_unchanged(&reply));
        let screen = decode_snapshot("d", &reply).unwrap();
        assert!(screen.elements.is_empty());
        assert_eq!(screen.hash.as_deref(), Some("empty"));
    }
}
