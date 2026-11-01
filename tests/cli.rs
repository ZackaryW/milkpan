//! The `milkpan` binary against a local mock of the GitHub API, plus a
//! depending binary that reuses the bucket commands with an extra provider.
use std::{
    io::Write,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    thread,
};

use milkpan::{
    BucketBuilder, ContentKind, Provider, Resolved, Spec,
    cli::{BucketCommands, BucketHandle},
};
use panned::{
    Error, ErrorCode,
    cli::{GenericCommands, Runner},
};
use serde_json::{Value, json};

const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn tarball() -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, data) in [
        ("octo-tools-aaaa/README.md", &b"hello"[..]),
        ("octo-tools-aaaa/docs/guide.md", b"guide"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, path, data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

struct Mock {
    server: Arc<tiny_http::Server>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Mock {
    fn start() -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let inner = server.clone();
        let thread = thread::spawn(move || {
            for request in inner.incoming_requests() {
                let url = request.url().to_string();
                let (status, body) = match url.as_str() {
                    "/repos/octo/tools" => (200, br#"{"default_branch": "main"}"#.to_vec()),
                    "/repos/octo/tools/commits/main" => (200, format!(r#"{{"sha": "{MAIN}"}}"#).into_bytes()),
                    u if u == format!("/repos/octo/tools/tarball/{MAIN}") => (200, tarball()),
                    _ => (404, b"{}".to_vec()),
                };
                let _ = request.respond(tiny_http::Response::from_data(body).with_status_code(status));
            }
        });
        Self {
            server,
            thread: Some(thread),
        }
    }

    fn api(&self) -> String {
        format!("http://{}", self.server.server_addr().to_ip().unwrap())
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn milkpan(home: &Path, api: &str, args: &[&str]) -> (i32, Value, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_milkpan"))
        .env("MILKPAN_HOME", home)
        .env("MILKPAN_GITHUB_API", api)
        .env("MILKPAN_PRINCIPAL", "user:test")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .arg("--format")
        .arg("json")
        .args(args)
        .output()
        .unwrap();
    let parse = |bytes: &[u8]| {
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(bytes).unwrap()
        }
    };
    (
        output.status.code().unwrap(),
        parse(&output.stdout),
        parse(&output.stderr),
    )
}

#[test]
fn acquire_a_snapshot_and_read_a_file_inside_it() {
    let (mock, home) = (Mock::start(), tempfile::tempdir().unwrap());
    let (code, out, err) = milkpan(home.path(), &mock.api(), &["acquire", "github:Octo/Tools"]);
    assert_eq!((code, &err), (0, &Value::Null), "{err}");
    let result = &out["result"];
    assert_eq!(result["lease"]["version"], MAIN);
    assert_eq!(result["lease"]["spec"], "github:octo/tools");
    assert_eq!(result["lease"]["digest"].as_str().unwrap().len(), 64);
    assert!(
        Path::new(result["path"].as_str().unwrap())
            .join("README.md")
            .is_file()
    );

    let id = result["lease"]["id"].as_str().unwrap().to_string();
    let (code, out, _) = milkpan(home.path(), &mock.api(), &["path", &id, "docs/guide.md"]);
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(out["result"]["path"].as_str().unwrap()).unwrap(),
        "guide"
    );
    let (code, _, err) = milkpan(
        home.path(),
        &mock.api(),
        &["path", "github:octo/tools", "missing.md"],
    );
    assert_eq!((code, err["error"]["code"].as_str()), (1, Some("not_found")));

    let (_, out, _) = milkpan(home.path(), &mock.api(), &["lease", "list"]);
    assert_eq!(out["result"]["leases"].as_array().unwrap().len(), 1);
    let (code, _, _) = milkpan(home.path(), &mock.api(), &["release", &id]);
    assert_eq!(code, 0);
    let (_, out, _) = milkpan(home.path(), &mock.api(), &["lease", "list"]);
    assert_eq!(out["result"]["leases"], json!([]));
}

#[test]
fn denials_use_the_shared_envelope() {
    let (mock, home) = (Mock::start(), tempfile::tempdir().unwrap());
    let (code, out, err) = milkpan(
        home.path(),
        &mock.api(),
        &["acquire", "github:octo/tools", "--scope", "ci"],
    );
    assert_eq!((code, out), (1, Value::Null));
    assert_eq!(err["ok"], false);
    assert_eq!(err["error"]["code"], "denied");
    assert_eq!(
        err["error"]["details"]["evidence"]["notes"],
        json!(["no statement matched"])
    );
}

#[test]
fn generic_commands_and_configuration_errors() {
    let (mock, home) = (Mock::start(), tempfile::tempdir().unwrap());
    let (code, out, _) = milkpan(home.path(), &mock.api(), &["scope", "list"]);
    assert_eq!((code, &out["result"]), (0, &json!({ "scopes": ["default"] })));
    std::fs::write(home.path().join("milkpan.toml"), "[[rule]\n").unwrap();
    let (code, _, err) = milkpan(home.path(), &mock.api(), &["lease", "list"]);
    assert_eq!(
        (code, err["error"]["code"].as_str()),
        (1, Some("invalid_request"))
    );
    let (code, out, _) = milkpan(home.path(), &mock.api(), &["protocol"]);
    assert_eq!(
        (code, &out["result"]["protocol"]),
        (0, &json!(1)),
        "protocol needs no configuration"
    );
}

/// A provider a depending crate would add.
#[derive(Clone, Default)]
struct Gitea(Arc<Mutex<bool>>);

impl Provider for Gitea {
    fn prefixes(&self) -> Vec<String> {
        vec!["gitea".into()]
    }
    fn kind(&self, _: &Spec) -> String {
        "file".into()
    }
    fn resolve(&self, spec: &Spec) -> Result<Resolved, Error> {
        if *self.0.lock().unwrap() && spec.source == "o/flaky" {
            return Err(Error::new(ErrorCode::Unavailable, "down"));
        }
        Ok(Resolved {
            version: "r1".into(),
            kind: ContentKind::File,
        })
    }
    fn fetch(&self, _: &Spec, _: &Resolved, out: &mut dyn Write) -> Result<(), Error> {
        out.write_all(b"gitea content")
            .map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))
    }
}

#[test]
fn a_depending_binary_reuses_the_commands_with_its_own_provider() {
    let home = tempfile::tempdir().unwrap();
    let gitea = Gitea::default();
    let provider = gitea.clone();
    let rules = Arc::new(
        panned::StaticRules::default()
            .allow_with(
                "default",
                &milkpan::config::ACTIONS,
                &["*"],
                vec![panned::Obligation::new("lease").with("refresh", "0s")],
            )
            .unwrap(),
    );
    let handle = BucketHandle::new(home.path(), move |home| {
        BucketBuilder::new(home)
            .provider(provider.clone())
            .authorizer(rules.clone(), rules.clone())
            .build()
    });
    let runner = Runner::new("saucepan", "0.0.0")
        .with(GenericCommands::new(handle.clone(), handle.clone()))
        .with(BucketCommands::new(handle));
    let run = |args: &[&str]| {
        let output = runner.run(["saucepan", "--format", "json"].iter().chain(args));
        let body = if output.stdout.is_empty() {
            &output.stderr
        } else {
            &output.stdout
        };
        (output.code, serde_json::from_str::<Value>(body).unwrap())
    };
    let (code, out) = run(&["acquire", "gitea:o/repo", "--principal", "app:saucepan"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["result"]["lease"]["spec"], "gitea:o/repo");
    run(&["acquire", "gitea:o/flaky", "--principal", "app:saucepan"]);

    // refresh 0s makes both leases due; one provider call fails, so the run is non-complete.
    *gitea.0.lock().unwrap() = true;
    let (code, out) = run(&["reacquire", "--due"]);
    assert_eq!(code, 2, "{out}");
    assert_eq!(out["result"]["reacquired"].as_array().unwrap().len(), 1);
    assert_eq!(out["result"]["failed"][0]["error"]["code"], "unavailable");
}

#[test]
fn the_default_home_is_dot_milkpan_in_the_user_home() {
    let user = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_milkpan"))
        .env_remove("MILKPAN_HOME")
        .env("HOME", user.path())
        .env("USERPROFILE", user.path())
        .args(["--format", "json", "reclaim"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        user.path().join(".milkpan").is_dir(),
        "the bucket is created in ~/.milkpan"
    );
}
