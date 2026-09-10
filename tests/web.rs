mod common;

use common::Fixture;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

fn unused_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start_web(fixture: &Fixture) -> (Child, Value) {
    assert!(fixture.project.exists());
    let port = unused_port();
    let mut child = fixture
        .command()
        .env("MX_TEST_BOOTED", "1")
        .args([
            "web",
            "--device",
            "Test Phone",
            "--port",
            &port.to_string(),
            "--fps",
            "20",
            "--quality",
            "70",
            "--scale",
            "0.5",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut json = String::new();
    loop {
        let line = lines.next().unwrap().unwrap();
        json.push_str(&line);
        if line == "}" {
            break;
        }
    }
    (child, serde_json::from_str(&json).unwrap())
}

fn request(url: &str, request: &[u8]) -> Vec<u8> {
    let address = url.strip_prefix("http://").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut stream = loop {
        match TcpStream::connect(address) {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("could not connect to web server: {error}"),
        }
    };
    stream.write_all(request).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    response
}

#[test]
fn web_serves_status_and_streams_directly_from_axe() {
    let fixture = Fixture::new();
    let (mut child, started) = start_web(&fixture);
    assert_eq!(started["device"], "test-device");
    assert_eq!(started["session_id"], "test-session");
    assert_eq!(started["fps"], 20);
    assert_eq!(started["quality"], 70);
    assert_eq!(started["scale"], 0.5);

    let response = request(
        started["url"].as_str().unwrap(),
        b"GET /api/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    let body = response.split_once("\r\n\r\n").unwrap().1;
    let status: Value = serde_json::from_str(body).unwrap();
    assert_eq!(status["width"].as_f64(), Some(390.0));
    assert_eq!(status["height"].as_f64(), Some(844.0));

    let calls = fixture.calls();
    assert!(calls.iter().any(|call| {
        call["program"] == "axe"
            && call["args"].as_array().is_some_and(|args| {
                args.first() == Some(&Value::String("stream-video".into()))
                    && args.contains(&Value::String("mjpeg".into()))
            })
    }));
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn web_sends_validated_input_through_axe() {
    let fixture = Fixture::new();
    let (mut child, started) = start_web(&fixture);
    let body = r#"{"x":10,"y":20}"#;
    let request_bytes = format!(
        "POST /api/tap HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let response = request(started["url"].as_str().unwrap(), request_bytes.as_bytes());
    assert!(response.starts_with(b"HTTP/1.1 204"));
    assert!(fixture.calls().iter().any(|call| {
        call["program"] == "axe"
            && call["args"].as_array().and_then(|args| args.first())
                == Some(&Value::String("tap".into()))
    }));

    child.kill().unwrap();
    child.wait().unwrap();
}
