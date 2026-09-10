use std::{
    env, fs,
    io::{self, Read, Write},
    path::Path,
    process, thread,
    time::Duration,
};

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn json_array(values: &[String]) -> String {
    format!("[{}]", values.iter().map(|v| format!("\"{}\"", escape(v))).collect::<Vec<_>>().join(","))
}

fn record(root: &Path, name: &str, args: &[String]) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(root.join("calls.jsonl")).unwrap();
    writeln!(file, "{{\"program\":\"{}\",\"args\":{}}}", escape(name), json_array(args)).unwrap();
}

fn labels(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("disabled.txt")).unwrap_or_default().lines().map(str::to_owned).collect()
}

fn save_labels(root: &Path, values: &mut Vec<String>) {
    values.sort();
    values.dedup();
    fs::write(root.join("disabled.txt"), values.join("\n")).unwrap();
}

fn main() {
    let root = std::path::PathBuf::from(env::var_os("MX_TEST_ROOT").unwrap());
    let name = env::args().next().and_then(|p| Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned())).unwrap();
    let args: Vec<String> = env::args().skip(1).collect();
    record(&root, &name, &args);
    match name.as_str() {
        "xcodebuild" => xcodebuild(&root, &args),
        "xcrun" => xcrun(&root, &args),
        "axe" => axe(&root, &args),
        _ => process::exit(2),
    }
}

fn xcodebuild(root: &Path, args: &[String]) {
    if args.iter().any(|a| a == "-list") {
        println!(r#"{{"project":{{"schemes":["Demo"]}}}}"#);
    } else if args.iter().any(|a| a == "-showBuildSettings") {
        let output = escape(&root.join("Build Products").to_string_lossy());
        println!(r#"[{{"buildSettings":{{"PRODUCT_TYPE":"com.apple.product-type.application","TARGET_BUILD_DIR":"{output}","FULL_PRODUCT_NAME":"Demo.app"}}}}]"#);
    } else {
        if env::var("MX_TEST_SLOW_BUILD").as_deref() == Ok("1") && !args.iter().any(|a| a.contains("test-device-2")) {
            fs::write(root.join("build.pid"), process::id().to_string()).unwrap();
            thread::sleep(Duration::from_secs(30));
        }
        if env::var("MX_TEST_FAIL_BUILD").as_deref() == Ok("1") {
            eprintln!("App.swift:7:1: error: intentional build failure");
            process::exit(65);
        }
        let app = root.join("Build Products/Demo.app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("Info.plist"), r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>dev.mx.test</string></dict></plist>"#).unwrap();
        println!("BUILD SUCCEEDED");
    }
}

fn xcrun(root: &Path, args: &[String]) {
    let op = args.get(1).map(String::as_str).unwrap_or("");
    match op {
        "list" => {
            let state = if env::var("MX_TEST_BOOTED").as_deref() == Ok("1") || root.join("booted").exists() { "Booted" } else { "Shutdown" };
            let mut devices = vec![format!(r#"{{"name":"Test Phone","udid":"test-device","state":"{state}","isAvailable":true,"deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro"}}"#)];
            if env::var("MX_TEST_TWO_DEVICES").as_deref() == Ok("1") {
                devices.push(r#"{"name":"Test Phone 2","udid":"test-device-2","state":"Shutdown","isAvailable":true,"deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro"}"#.into());
            }
            println!(r#"{{"devices":{{"com.apple.CoreSimulator.SimRuntime.iOS-26-5":[{}]}}}}"#, devices.join(","));
        }
        "create" => println!("00000000-0000-4000-8000-000000000001"),
        "boot" => { fs::write(root.join("booted"), "").unwrap(); }
        "pbcopy" => { let mut input = String::new(); io::stdin().read_to_string(&mut input).unwrap(); fs::write(root.join("pasteboard.txt"), input).unwrap(); }
        "launch" => println!("dev.mx.test: 4321"),
        "io" => {
            let path = Path::new(args.last().unwrap());
            if path.exists() { eprintln!("simctl requires a new output file"); process::exit(1); }
            fs::write(path, b"\x89PNG\r\n\x1a\n").unwrap();
        }
        "spawn" if args.get(3).map(String::as_str) == Some("/bin/sh") => batch_transition(root, args),
        "spawn" if args.get(3).map(String::as_str) == Some("launchctl") => launchctl(root, args),
        "spawn" => for i in 0..250 { println!(r#"{{"eventMessage":"line {i}"}}"#); },
        _ => {}
    }
}

fn batch_transition(root: &Path, args: &[String]) {
    let script = args.get(5).map(String::as_str).unwrap_or("");
    if script.contains(" bootout \"user/501/$label\"") { return; }
    let action = if script.contains("launchctl\" disable") { "disable" } else if script.contains("launchctl\" enable") { "enable" } else { eprintln!("invalid batch transition script"); process::exit(2) };
    let start = script.find("for label in").map(|i| i + 12).unwrap_or(0);
    let end = script[start..].find("; do").map(|i| start + i).unwrap_or(start);
    if start == end { eprintln!("invalid batch transition script"); process::exit(2); }
    let mut current = labels(root);
    for (index, label) in script[start..end].split_whitespace().enumerate() {
        if env::var("MX_TEST_PROFILE_FAIL").as_deref() == Ok("1") && index >= 3 { save_labels(root, &mut current); eprintln!("injected batch transition failure"); process::exit(1); }
        if action == "disable" { current.push(label.into()); } else { current.retain(|v| v != label); }
    }
    save_labels(root, &mut current);
}

fn launchctl(root: &Path, args: &[String]) {
    let lock = root.join("disabled.lock");
    while fs::create_dir(&lock).is_err() { thread::sleep(Duration::from_millis(2)); }
    let mut current = labels(root);
    let action = args.get(4).map(String::as_str).unwrap_or("");
    if action == "print-disabled" {
        current.sort();
        for label in current { println!(r#""{label}" => disabled"#); }
    } else if let Some(label) = args.get(5).and_then(|v| v.strip_prefix("system/")) {
        if action == "disable" {
            if env::var("MX_TEST_PROFILE_FAIL").as_deref() == Ok("1") && current.len() >= 3 { let _ = fs::remove_dir(&lock); eprintln!("injected service transition failure"); process::exit(1); }
            current.push(label.into());
        } else if action == "enable" { current.retain(|v| v != label); }
        save_labels(root, &mut current);
    }
    fs::remove_dir(lock).unwrap();
}

fn axe(root: &Path, args: &[String]) {
    match args.first().map(String::as_str).unwrap_or("") {
        "--version" => println!("1.8.0"),
        "describe-ui" => {
            let pid = env::var("MX_TEST_FOREGROUND_PID").unwrap_or_else(|_| "4321".into());
            println!(r#"[{{"type":"Application","pid":{pid},"frame":{{"x":0,"y":0,"width":390,"height":844}},"children":[{{"type":"Button","AXLabel":"Continue","AXUniqueId":"first"}},{{"type":"Button","AXLabel":"Continue","AXUniqueId":"second"}},{{"type":"TextField","AXLabel":"Name","AXUniqueId":"name","AXValue":""}}]}}]"#);
        }
        "stream-video" => loop {
            io::stdout()
                .write_all(b"\xff\xd8test-frame\xff\xd9")
                .unwrap();
            io::stdout().flush().unwrap();
            thread::sleep(Duration::from_millis(20));
        },
        "tap" | "swipe" => {}
        "key-combo" => { fs::copy(root.join("pasteboard.txt"), root.join("typed.txt")).unwrap(); }
        "type" => { let mut input = String::new(); io::stdin().read_to_string(&mut input).unwrap(); fs::write(root.join("typed.txt"), input).unwrap(); }
        _ => {}
    }
}
