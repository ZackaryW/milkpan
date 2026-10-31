//! The self-management flow through the binary: acquire a release, link it with
//! a shim, publish a new release, reacquire, and the next start runs it.
use std::{
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    thread,
};

use serde_json::{Value, json};

#[cfg(windows)]
const TOOL: &str = "tool.cmd";
#[cfg(not(windows))]
const TOOL: &str = "tool";

fn release_archive(version: u32) -> Vec<u8> {
    let body = if cfg!(windows) {
        format!("@echo v{version} %*\r\n")
    } else {
        format!("#!/bin/sh\necho v{version} \"$@\"\n")
    };
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o755);
    builder
        .append_data(&mut header, format!("tool-v{version}/{TOOL}"), body.as_bytes())
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

/// A mock GitHub API whose latest release of octo/tool can be changed.
struct Releases {
    server: Arc<tiny_http::Server>,
    latest: Arc<Mutex<u32>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Releases {
    fn start() -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let latest = Arc::new(Mutex::new(1));
        let (inner, current) = (server.clone(), latest.clone());
        let thread = thread::spawn(move || {
            for request in inner.incoming_requests() {
                let version = *current.lock().unwrap();
                let url = request.url().to_string();
                let (status, body) = if url == "/repos/octo/tool/releases/latest" {
                    let id = 100 + version;
                    (
                        200,
                        format!(r#"{{"assets": [{{"id": {id}, "name": "tool-v{version}.tar.gz"}}]}}"#)
                            .into_bytes(),
                    )
                } else if let Some(id) = url.strip_prefix("/repos/octo/tool/releases/assets/") {
                    (200, release_archive(id.parse::<u32>().unwrap() - 100))
                } else {
                    (404, b"{}".to_vec())
                };
                let _ = request.respond(tiny_http::Response::from_data(body).with_status_code(status));
            }
        });
        Self {
            server,
            latest,
            thread: Some(thread),
        }
    }

    fn api(&self) -> String {
        format!("http://{}", self.server.server_addr().to_ip().unwrap())
    }
}

impl Drop for Releases {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn milkpan(home: &Path, api: &str, args: &[&str]) -> (i32, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_milkpan"))
        .env("MILKPAN_HOME", home)
        .env("MILKPAN_GITHUB_API", api)
        .env("MILKPAN_PRINCIPAL", "user:test")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .args(["--format", "json"])
        .args(args)
        .output()
        .unwrap();
    let body = if output.stdout.is_empty() {
        output.stderr
    } else {
        output.stdout
    };
    (
        output.status.code().unwrap(),
        serde_json::from_slice(&body).unwrap(),
    )
}

fn run(shim: &Path) -> String {
    let output = if cfg!(windows) {
        Command::new("cmd")
            .arg("/C")
            .arg(shim)
            .arg("hello")
            .output()
            .unwrap()
    } else {
        Command::new(shim).arg("hello").output().unwrap()
    };
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn a_linked_release_updates_on_the_next_start() {
    let (releases, home) = (Releases::start(), tempfile::tempdir().unwrap());
    // refresh 0s makes the lease due immediately, so `reacquire --due` picks up the new release.
    std::fs::write(
        home.path().join("milkpan.toml"),
        r#"
[[rule]]
scope = "default"
effect = "allow"
actions = ["acquire", "reacquire", "read", "release", "link"]
resources = ["*"]
obligations = [{ name = "lease", params = { refresh = "0s" } }]
"#,
    )
    .unwrap();
    let api = releases.api();

    let (code, out) = milkpan(
        home.path(),
        &api,
        &["acquire", "github-release:octo/tool@latest#tool-*"],
    );
    assert_eq!(code, 0, "{out}");
    let lease = out["result"]["lease"]["id"].as_str().unwrap().to_string();
    let (code, out) = milkpan(
        home.path(),
        &api,
        &["link", &lease, "--name", "tool", "--file", TOOL],
    );
    assert_eq!(code, 0, "{out}");
    let shim = Path::new(out["result"]["shim"].as_str().unwrap()).to_path_buf();
    assert_eq!(run(&shim), "v1 hello");

    *releases.latest.lock().unwrap() = 2;
    let (code, out) = milkpan(home.path(), &api, &["reacquire", "--due"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(run(&shim), "v2 hello", "the next start runs the new release");

    let (code, out) = milkpan(home.path(), &api, &["slot", "list"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        out["result"]["slots"],
        json!([{ "name": "tool", "current": "102", "available": ["102"] }])
    );
    let (_, out) = milkpan(home.path(), &api, &["link", "list"]);
    assert_eq!(out["result"]["links"][0]["name"], "tool");
    let (code, _) = milkpan(home.path(), &api, &["unlink", "tool"]);
    assert_eq!(code, 0);
    assert!(!shim.exists());
}
