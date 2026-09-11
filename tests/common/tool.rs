use std::{
    env, fs,
    io::{self, Read},
    path::Path,
    process, thread,
    time::Duration,
};
use process::Stdio;

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn json_array(values: &[String]) -> String {
    format!("[{}]", values.iter().map(|v| format!("\"{}\"", escape(v))).collect::<Vec<_>>().join(","))
}

fn record(root: &Path, name: &str, args: &[String]) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(root.join("calls.jsonl")).unwrap();
    let mut locked = false;
    for _ in 0..500 {
        if file.try_lock().is_ok() { locked = true; break; }
        thread::sleep(Duration::from_millis(2));
    }
    writeln!(file, "{{\"program\":\"{}\",\"args\":{}}}", escape(name), json_array(args)).unwrap();
    if locked { let _ = file.unlock(); }
}

/// Emulate the real simulator process tree: a `launchd_sim` root carrying the
/// device UDID in its command line, with a transient helper child. These are
/// real processes in their own process group, so the unmodified metrics.rs
/// proc_pid_rusage / proc_pidpath / kill logic observes and trims them.
fn spawn_sim_tree(root: &Path) {
    use std::os::unix::process::CommandExt;
    let exe = env::current_exe().unwrap();
    let sim_bin = root.join("launchd_sim");
    fs::copy(&exe, &sim_bin).unwrap();
    let sim = process::Command::new(&sim_bin)
        .arg0("launchd_sim")
        .arg("__sim-tree")
        .arg(root.to_string_lossy().as_ref())
        .arg("test-device")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    fs::write(root.join("sim-tree.pid"), sim.id().to_string()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !root.join("sim-helper.pid").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "simulator tree helper did not start"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// Long-lived stand-in for a simulator process. Spawns a transient helper
/// child, then exits when its PID file is removed (simulator shutdown).
fn sim_tree_process(root: &Path) {
    use std::os::unix::process::CommandExt;
    let exe = env::current_exe().unwrap();
    let helper_bin = root.join("MTLCompilerService");
    fs::copy(&exe, &helper_bin).unwrap();
    let helper = process::Command::new(&helper_bin)
        .arg0("MTLCompilerService")
        .arg("__sim-tree-helper")
        .arg(root.to_string_lossy().as_ref())
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    fs::write(root.join("sim-helper.pid"), helper.id().to_string()).unwrap();
    let mut helper = helper;
    loop {
        if !root.join("sim-tree.pid").exists() {
            process::exit(0);
        }
        let _ = helper.try_wait();
        thread::sleep(Duration::from_millis(20));
    }
}

/// Transient helper: dies on SIGTERM like the real Metal compiler service.
fn sim_tree_helper(root: &Path) {
    loop {
        if !root.join("sim-helper.pid").exists() {
            process::exit(0);
        }
        thread::sleep(Duration::from_millis(20));
    }
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
    if args.first().map(String::as_str) == Some("__sim-tree") {
        sim_tree_process(&root);
        return;
    }
    if args.first().map(String::as_str) == Some("__sim-tree-helper") {
        sim_tree_helper(&root);
        return;
    }
    record(&root, &name, &args);
    match name.as_str() {
        "xcodebuild" => xcodebuild(&root, &args),
        "xcrun" => xcrun(&root, &args),
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
        "boot" => {
            fs::write(root.join("booted"), "").unwrap();
            if env::var("MX_TEST_SIM_TREE").as_deref() == Ok("1") && !root.join("sim-tree.pid").exists() {
                spawn_sim_tree(root);
            }
        }
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
