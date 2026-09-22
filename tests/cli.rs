use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Output},
    thread,
};

fn binary() -> std::ffi::OsString {
    std::env::var_os("NINKO_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_ninko").into())
}

#[cfg(target_os = "linux")]
#[test]
fn linux_respects_xdg_data_home() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("io.github.clash-verge-rev.clash-verge-rev");
    std::fs::create_dir_all(app.join("profiles")).unwrap();
    std::fs::write(
        app.join("profiles.yaml"),
        "items:\n- uid: Merge\n  type: merge\n  file: Merge.yaml\n",
    )
    .unwrap();
    std::fs::write(app.join("profiles/Merge.yaml"), "# Clash\n").unwrap();
    let output = Command::new(binary())
        .env_remove("CLASH_DATA_DIR")
        .env("XDG_DATA_HOME", dir.path())
        .arg("merge-path")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        app.join("profiles/Merge.yaml").to_str().unwrap()
    );
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("profiles")).unwrap();
    std::fs::write(dir.path().join("profiles.yaml"), "current: active\nitems:\n- uid: Merge\n  type: merge\n  file: custom.yaml\n- uid: active\n  type: local\n  name: 本地配置\n  file: active.yaml\n").unwrap();
    std::fs::write(dir.path().join("profiles/custom.yaml"), "# Clash\n").unwrap();
    dir
}

fn run(args: &[&str], replies: Vec<(&'static str, Value)>) -> (Output, Vec<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        for (expected, response) in replies {
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(s) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "等待请求超时：{expected}"
                        );
                        thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut data = Vec::new();
            loop {
                let mut buf = [0; 4096];
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                data.extend_from_slice(&buf[..n]);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..end]);
                    let length = headers
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|s| s.parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let request = String::from_utf8(data).unwrap();
            assert!(request.starts_with(expected), "{request}");
            let body = response.to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            requests.push(request);
        }
        requests
    });
    let dir = fixture();
    let output = Command::new(binary())
        .env_remove("CLASH_SOCK")
        .env_remove("CLASH_API")
        .env_remove("CLASH_SECRET")
        .arg("--api")
        .arg(addr)
        .arg("--data-dir")
        .arg(dir.path())
        .args(args)
        .output()
        .unwrap();
    (output, server.join().unwrap())
}

fn runtime_replies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "GET /providers/proxies ",
            json!({"providers": {
                "订阅": {"vehicleType": "HTTP", "proxies": [{"name": "香港/01", "type": "Shadowsocks"}]},
                "default": {"vehicleType": "Compatible", "proxies": [{"name": "香港/01"}]}
            }}),
        ),
        (
            "GET /proxies ",
            json!({"proxies": {
                "AI 专用": {"name": "AI 专用", "type": "Selector", "all": ["香港/01", "DIRECT"], "now": "DIRECT"},
                "香港/01": {"name": "香港/01", "type": "Shadowsocks"},
                "DIRECT": {"name": "DIRECT", "type": "Direct"}
            }}),
        ),
    ]
}

#[test]
fn lists_real_providers_without_synthetic_duplicates() {
    let (out, _) = run(&["list", "--json"], runtime_replies());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["current_profile"]["uid"], "active");
    assert_eq!(v["providers"].as_object().unwrap().len(), 1);
    assert_eq!(v["providers"]["订阅"]["proxies"][0]["name"], "香港/01");
}

#[test]
fn switch_encodes_names_and_verifies_selection() {
    let mut replies = runtime_replies();
    replies.push(("PUT /proxies/AI%20%E4%B8%93%E7%94%A8 ", Value::Null));
    replies.push((
        "GET /proxies/AI%20%E4%B8%93%E7%94%A8 ",
        json!({"now": "香港/01"}),
    ));
    let (out, requests) = run(&["switch", "AI 专用", "香港/01"], replies);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(requests[2].contains("{\"name\":\"香港/01\"}"));
}

#[test]
fn rejects_invalid_selection_without_mutation() {
    let (out, requests) = run(&["switch", "AI 专用", "不存在"], runtime_replies());
    assert!(!out.status.success());
    assert_eq!(requests.len(), 2);
}

#[test]
fn tests_provider_node_using_provider_scoped_endpoint() {
    let mut replies = runtime_replies();
    replies.push((
        "GET /providers/proxies/%E8%AE%A2%E9%98%85/%E9%A6%99%E6%B8%AF%2F01/healthcheck?",
        json!({"delay": 42}),
    ));
    let (out, _) = run(&["test", "--json"], replies);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["delay_ms"], 42);
}

#[test]
fn all_failed_tests_exit_nonzero_and_keep_json() {
    let mut replies = runtime_replies();
    replies.push(("GET /providers/proxies/", json!({"delay": 0})));
    let (out, _) = run(&["test", "--json"], replies);
    assert!(!out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v[0]["delay_ms"].is_null());
    assert!(v[0]["error"].is_string());
}

#[test]
fn merge_path_uses_global_metadata_without_controller() {
    let dir = fixture();
    let out = Command::new(binary())
        .arg("--data-dir")
        .arg(dir.path())
        .arg("merge-path")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        dir.path()
            .join("profiles/custom.yaml")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
}

#[test]
fn group_test_skips_direct_and_uses_encoded_node_path() {
    let mut replies = runtime_replies();
    replies.push((
        "GET /proxies/%E9%A6%99%E6%B8%AF%2F01/delay?",
        json!({"delay": 25}),
    ));
    let (out, _) = run(&["test", "--group", "AI 专用", "--json"], replies);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["delay_ms"], 25);
}

#[test]
fn switch_reports_readback_mismatch() {
    let mut replies = runtime_replies();
    replies.push(("PUT /proxies/", Value::Null));
    replies.push(("GET /proxies/", json!({"now": "DIRECT"})));
    let (out, _) = run(&["switch", "AI 专用", "香港/01"], replies);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("读回结果不一致"));
}
