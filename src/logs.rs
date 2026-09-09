use crate::{process, runtime, session};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize)]
pub struct Entry {
    pub sequence: u64,
    pub text: String,
}
#[derive(Default)]
struct Buffer {
    entries: VecDeque<Entry>,
    next: u64,
    bytes: usize,
    done: bool,
    error: Option<String>,
}
struct Stream {
    buffer: Arc<Mutex<Buffer>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[derive(Clone, Default)]
pub struct Manager {
    streams: Arc<Mutex<HashMap<String, Stream>>>,
}
#[derive(Serialize)]
pub struct Batch {
    pub entries: Vec<Entry>,
    pub cursor: u64,
    pub dropped: bool,
    pub done: bool,
    pub error: Option<String>,
}
impl Manager {
    pub async fn start(&self, device: &str, pid: u32) -> Result<String> {
        let device = runtime::booted_device(device).await?;
        let bound = session::active(&device.udid)?;
        anyhow::ensure!(
            bound.pid == pid,
            "Log PID does not belong to the active Mx session"
        );
        let mut streams = self.streams.lock().unwrap();
        anyhow::ensure!(
            streams.len() < 8,
            "At most 8 live log streams per server; stop an existing stream first"
        );
        let id = uuid::Uuid::new_v4().to_string();
        let buffer = Arc::new(Mutex::new(Buffer::default()));
        let sink = buffer.clone();
        let task = tokio::spawn(async move {
            let args = process::strings(&[
                "simctl",
                "spawn",
                &device.udid,
                "log",
                "stream",
                "--style",
                "ndjson",
                "--level",
                "debug",
                "--predicate",
                &format!("processIdentifier == {pid}"),
            ]);
            let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
            let pump = process::pump("xcrun", &args, sender);
            let consume = async {
                while let Some(text) = receiver.recv().await {
                    let mut buffer = sink.lock().unwrap();
                    buffer.next += 1;
                    let sequence = buffer.next;
                    buffer.bytes += text.len();
                    buffer.entries.push_back(Entry { sequence, text });
                    while buffer.entries.len() > 1000 || buffer.bytes > 1024 * 1024 {
                        if let Some(entry) = buffer.entries.pop_front() {
                            buffer.bytes -= entry.text.len();
                        }
                    }
                }
            };
            let (result, _) = tokio::join!(pump, consume);
            let mut buffer = sink.lock().unwrap();
            buffer.done = true;
            buffer.error = result.err().map(|e| e.to_string());
        });
        streams.insert(id.clone(), Stream { buffer, task });
        Ok(id)
    }
    pub fn read(&self, id: &str, after: u64) -> Result<Batch> {
        let streams = self.streams.lock().unwrap();
        let stream = streams.get(id).context("Unknown live log stream")?;
        let buffer = stream.buffer.lock().unwrap();
        anyhow::ensure!(after <= buffer.next, "Log cursor is ahead of this stream");
        let mut bytes = 0;
        let entries: Vec<_> = buffer
            .entries
            .iter()
            .filter(|e| e.sequence > after)
            .take(200)
            .take_while(|e| {
                bytes += e.text.len();
                bytes <= 64 * 1024
            })
            .cloned()
            .collect();
        let cursor = entries.last().map(|e| e.sequence).unwrap_or(after);
        Ok(Batch {
            entries,
            cursor,
            dropped: buffer
                .entries
                .front()
                .is_some_and(|e| after + 1 < e.sequence),
            done: buffer.done,
            error: buffer.error.clone(),
        })
    }
    pub fn stop(&self, id: &str) -> Result<()> {
        self.streams
            .lock()
            .unwrap()
            .remove(id)
            .context("Unknown live log stream")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cursor_gaps_and_batch_byte_limits_are_explicit() {
        let manager = Manager::default();
        let entries: VecDeque<_> = (101..=110)
            .map(|sequence| Entry {
                sequence,
                text: "x".repeat(16 * 1024),
            })
            .collect();
        let buffer = Arc::new(Mutex::new(Buffer {
            entries,
            next: 110,
            bytes: 160 * 1024,
            done: true,
            error: None,
        }));
        manager.streams.lock().unwrap().insert(
            "s".into(),
            Stream {
                buffer,
                task: tokio::spawn(async {}),
            },
        );
        let first = manager.read("s", 0).unwrap();
        assert!(first.dropped);
        assert_eq!(first.entries.len(), 4);
        assert_eq!(first.cursor, 104);
        assert!(!manager.read("s", first.cursor).unwrap().dropped);
        assert!(manager.read("s", 111).is_err());
        manager.stop("s").unwrap();
        assert!(manager.read("s", 0).is_err());
    }
}
