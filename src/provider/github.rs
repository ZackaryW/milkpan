//! The built-in GitHub provider: repository snapshots, single files and
//! release assets at exact versions, over the REST API.
//!
//! The token is borrowed from `GH_TOKEN`, `GITHUB_TOKEN` or `gh auth token`
//! and never stored or printed. `gh` is only asked when talking to
//! `api.github.com`, so a custom API base (`MILKPAN_GITHUB_API`) never
//! receives the user's GitHub login.
use std::{fmt, io, io::Write, process::Command, sync::OnceLock, time::Duration};

use panned::{Error, ErrorCode, core::matches};
use serde::Deserialize;

use crate::{
    code,
    provider::{Provider, Resolved},
    spec::Spec,
    store::ContentKind,
};

/// The public GitHub API.
pub const DEFAULT_API: &str = "https://api.github.com";

/// Validate a GitHub spec and fold the case-insensitive parts (owner and repository).
pub fn canonicalize(mut spec: Spec) -> Result<Spec, Error> {
    let invalid = |why: &str| {
        Error::new(
            ErrorCode::InvalidRequest,
            format!("invalid GitHub spec {spec}: {why}"),
        )
        .with_detail("spec", spec.canonical())
    };
    let parts: Vec<&str> = spec.source.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
        return Err(invalid("source must be owner/repo"));
    }
    match spec.prefix.as_str() {
        "github" if spec.asset.is_some() => return Err(invalid("only github-release specs take #asset")),
        "github-release" if spec.path.is_some() => return Err(invalid("release specs take no path")),
        "github-release" if spec.selector.is_none() || spec.asset.is_none() => {
            return Err(invalid("release specs need @tag (or @latest) and #asset"));
        }
        "github" | "github-release" => {}
        _ => return Err(invalid("not a GitHub prefix")),
    }
    spec.source = spec.source.to_ascii_lowercase();
    Ok(spec)
}

/// A borrowed token. Its `Debug` and `Display` never show the value.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

fn borrow_token(api: &str) -> Option<Token> {
    for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = std::env::var(name)
            && !value.trim().is_empty()
        {
            return Some(Token(value.trim().to_string()));
        }
    }
    if api != DEFAULT_API {
        return None;
    }
    let output = Command::new("gh").args(["auth", "token"]).output().ok()?;
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !token.is_empty()).then_some(Token(token))
}

/// The GitHub provider, for prefixes `github` and `github-release`.
pub struct GitHub {
    api: String,
    agent: ureq::Agent,
    token: OnceLock<Option<Token>>,
}

impl fmt::Debug for GitHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHub")
            .field("api", &self.api)
            .field("token", &self.token.get())
            .finish()
    }
}

impl GitHub {
    /// A provider for the API at `api`, borrowing credentials on first use.
    pub fn new(api: impl Into<String>) -> Self {
        Self {
            api: api.into().trim_end_matches('/').to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(15))
                .timeout_read(Duration::from_secs(300))
                .user_agent(concat!("milkpan/", env!("CARGO_PKG_VERSION")))
                .build(),
            token: OnceLock::new(),
        }
    }

    /// The provider for `MILKPAN_GITHUB_API`, or the public API.
    pub fn from_env() -> Self {
        Self::new(std::env::var("MILKPAN_GITHUB_API").unwrap_or_else(|_| DEFAULT_API.into()))
    }

    /// Use this token (or none) instead of borrowing one.
    pub fn with_token(self, token: Option<String>) -> Self {
        let _ = self.token.set(token.map(Token));
        self
    }

    fn get(
        &self,
        spec: &Spec,
        path: &str,
        accept: &str,
        query: &[(&str, &str)],
    ) -> Result<ureq::Response, Error> {
        let mut request = self
            .agent
            .get(&format!("{}{path}", self.api))
            .set("Accept", accept);
        for (key, value) in query {
            request = request.query(key, value);
        }
        if let Some(Token(token)) = self.token.get_or_init(|| borrow_token(&self.api)) {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        request.call().map_err(|error| failure(spec, error))
    }

    fn json<T: for<'de> Deserialize<'de>>(&self, spec: &Spec, path: &str) -> Result<T, Error> {
        self.get(spec, path, "application/vnd.github+json", &[])?
            .into_json()
            .map_err(|e| {
                Error::new(
                    code("fetch_failed"),
                    format!("unexpected GitHub response for {spec}: {e}"),
                )
                .with_detail("spec", spec.canonical())
            })
    }

    fn commit(&self, spec: &Spec) -> Result<String, Error> {
        let reference = match &spec.selector {
            Some(selector) => selector.clone(),
            None => {
                self.json::<Repository>(spec, &format!("/repos/{}", spec.source))?
                    .default_branch
            }
        };
        Ok(self
            .json::<Commit>(spec, &format!("/repos/{}/commits/{reference}", spec.source))?
            .sha)
    }

    fn asset(&self, spec: &Spec) -> Result<String, Error> {
        let tag = spec
            .selector
            .as_deref()
            .expect("canonical release specs have a tag");
        let path = match tag {
            "latest" => format!("/repos/{}/releases/latest", spec.source),
            tag => format!("/repos/{}/releases/tags/{tag}", spec.source),
        };
        let pattern = spec
            .asset
            .as_deref()
            .expect("canonical release specs have an asset");
        let release: Release = self.json(spec, &path)?;
        let matching: Vec<&Asset> = release
            .assets
            .iter()
            .filter(|a| matches(pattern, &a.name))
            .collect();
        match matching.as_slice() {
            [asset] => Ok(asset.id.to_string()),
            [] => Err(Error::new(
                ErrorCode::NotFound,
                format!("no asset of {spec} matches {pattern:?}"),
            )
            .with_detail("spec", spec.canonical())),
            many => Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "asset pattern {pattern:?} matches {} assets of {spec}",
                    many.len()
                ),
            )
            .with_detail("spec", spec.canonical())
            .with_detail(
                "matches",
                panned::Value::List(many.iter().map(|a| a.name.clone()).collect()),
            )),
        }
    }
}

#[derive(Deserialize)]
struct Repository {
    default_branch: String,
}

#[derive(Deserialize)]
struct Commit {
    sha: String,
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    id: u64,
    name: String,
}

/// Map a request failure to a stable code. Messages never include headers.
fn failure(spec: &Spec, error: ureq::Error) -> Error {
    match error {
        ureq::Error::Status(status, response) => {
            let limited =
                status == 429 || (status == 403 && response.header("x-ratelimit-remaining") == Some("0"));
            let (code, message) = match status {
                404 => (ErrorCode::NotFound, format!("{spec} was not found on GitHub")),
                _ if limited => (
                    ErrorCode::Unavailable,
                    format!("GitHub rate limit reached while fetching {spec}"),
                ),
                _ => (
                    code("fetch_failed"),
                    format!("GitHub answered {status} for {spec}"),
                ),
            };
            Error::new(code, message)
                .with_detail("spec", spec.canonical())
                .with_detail("status", f64::from(status))
        }
        ureq::Error::Transport(transport) => Error::new(
            ErrorCode::Unavailable,
            format!("cannot reach GitHub for {spec}: {}", transport.kind()),
        )
        .with_detail("spec", spec.canonical()),
    }
}

impl Provider for GitHub {
    fn prefixes(&self) -> Vec<String> {
        vec!["github".into(), "github-release".into()]
    }

    fn canonicalize(&self, spec: Spec) -> Result<Spec, Error> {
        canonicalize(spec)
    }

    fn kind(&self, spec: &Spec) -> String {
        match (spec.prefix.as_str(), &spec.path) {
            ("github-release", _) => "release",
            (_, Some(_)) => "file",
            _ => "snapshot",
        }
        .into()
    }

    fn resolve(&self, spec: &Spec) -> Result<Resolved, Error> {
        Ok(match (spec.prefix.as_str(), &spec.path) {
            ("github-release", _) => Resolved {
                version: self.asset(spec)?,
                kind: ContentKind::File,
            },
            (_, Some(_)) => Resolved {
                version: self.commit(spec)?,
                kind: ContentKind::File,
            },
            _ => Resolved {
                version: self.commit(spec)?,
                kind: ContentKind::Archive,
            },
        })
    }

    fn fetch(&self, spec: &Spec, resolved: &Resolved, out: &mut dyn Write) -> Result<(), Error> {
        let (src, version) = (&spec.source, &resolved.version);
        let response = match (spec.prefix.as_str(), &spec.path) {
            ("github-release", _) => self.get(
                spec,
                &format!("/repos/{src}/releases/assets/{version}"),
                "application/octet-stream",
                &[],
            )?,
            (_, Some(path)) => self.get(
                spec,
                &format!("/repos/{src}/contents/{path}"),
                "application/vnd.github.raw",
                &[("ref", version)],
            )?,
            _ => self.get(
                spec,
                &format!("/repos/{src}/tarball/{version}"),
                "application/vnd.github+json",
                &[],
            )?,
        };
        io::copy(&mut response.into_reader(), out)
            .map(|_| ())
            .map_err(|e| {
                Error::new(ErrorCode::Unavailable, format!("download of {spec} failed: {e}"))
                    .with_detail("spec", spec.canonical())
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        thread,
    };

    use super::*;
    use crate::store::{Store, tarball};

    const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// Each request's URL and Authorization header.
    type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

    /// A local stand-in for the GitHub API that records each request's URL and
    /// Authorization header.
    struct Mock {
        server: Arc<tiny_http::Server>,
        seen: Seen,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Mock {
        fn start() -> Self {
            let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let (server2, seen2) = (server.clone(), seen.clone());
            let thread = thread::spawn(move || {
                for request in server2.incoming_requests() {
                    let auth = request
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Authorization"))
                        .map(|h| h.value.to_string());
                    seen2.lock().unwrap().push((request.url().to_string(), auth));
                    let (status, body, limited) = route(request.url());
                    let mut response = tiny_http::Response::from_data(body).with_status_code(status);
                    if limited {
                        response
                            .add_header(tiny_http::Header::from_bytes("x-ratelimit-remaining", "0").unwrap());
                    }
                    let _ = request.respond(response);
                }
            });
            Self {
                server,
                seen,
                thread: Some(thread),
            }
        }

        fn api(&self) -> String {
            format!("http://{}", self.server.server_addr().to_ip().unwrap())
        }

        fn urls(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .map(|(url, _)| url.clone())
                .collect()
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

    fn route(url: &str) -> (u16, Vec<u8>, bool) {
        let json = |text: &str| (200, text.as_bytes().to_vec(), false);
        match url {
            "/repos/octo/tools" => json(r#"{"default_branch": "main"}"#),
            "/repos/octo/tools/commits/main" => json(&format!(r#"{{"sha": "{MAIN}"}}"#)),
            u if u == format!("/repos/octo/tools/tarball/{MAIN}") => (
                200,
                tarball(&[
                    ("octo-tools-aaaa/README.md", b"hello"),
                    ("octo-tools-aaaa/src/lib.rs", b"//"),
                ]),
                false,
            ),
            u if u == format!("/repos/octo/tools/contents/README.md?ref={MAIN}") => {
                (200, b"hello".to_vec(), false)
            }
            "/repos/octo/tools/releases/latest" => json(
                r#"{"assets": [{"id": 11, "name": "tools-linux.tar.gz"}, {"id": 12, "name": "tools-mac.tar.gz"}]}"#,
            ),
            "/repos/octo/tools/releases/assets/11" => (200, b"linux build".to_vec(), false),
            "/repos/octo/limited" => (403, b"{}".to_vec(), true),
            "/repos/octo/broken" => (500, b"{}".to_vec(), false),
            _ => (404, b"{}".to_vec(), false),
        }
    }

    fn spec(text: &str) -> Spec {
        canonicalize(Spec::parse(text).unwrap()).unwrap()
    }

    fn canonical(text: &str) -> Result<String, Error> {
        canonicalize(Spec::parse(text)?).map(|s| s.canonical())
    }

    #[test]
    fn owner_and_repo_fold_case_but_paths_and_refs_do_not() {
        assert_eq!(
            canonical("github:Octo/Tools@Main").unwrap(),
            "github:octo/tools@Main"
        );
        assert_eq!(
            canonical("GITHUB:octo/tools:Docs/A.md").unwrap(),
            "github:octo/tools:Docs/A.md"
        );
        assert_eq!(
            canonical("github-release:Octo/Tools@latest#T-*").unwrap(),
            "github-release:octo/tools@latest#T-*"
        );
    }

    #[test]
    fn invalid_github_specs() {
        for text in [
            "github:octo",
            "github:octo/tools/x",
            "github:o/r#asset",
            "github-release:o/r#a",
            "github-release:o/r@v1",
            "github-release:o/r:p@v1#a",
        ] {
            assert_eq!(
                canonical(text).unwrap_err().code,
                ErrorCode::InvalidRequest,
                "{text}"
            );
        }
    }

    fn fetch(github: &GitHub, spec: &Spec, resolved: &Resolved) -> Vec<u8> {
        let mut bytes = Vec::new();
        github.fetch(spec, resolved, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn snapshots_resolve_the_default_branch_and_carry_no_history() {
        let mock = Mock::start();
        let github = GitHub::new(mock.api()).with_token(Some("secret-token".into()));
        let spec = spec("github:octo/tools");
        let resolved = github.resolve(&spec).unwrap();
        assert_eq!(
            resolved,
            Resolved {
                version: MAIN.into(),
                kind: ContentKind::Archive
            }
        );
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let digest = store.ingest(|out| github.fetch(&spec, &resolved, out)).unwrap();
        let tree = store.tree(&digest).unwrap();
        assert_eq!(std::fs::read_to_string(tree.join("README.md")).unwrap(), "hello");
        assert!(!tree.join(".git").exists());
        let seen = mock.seen.lock().unwrap();
        assert!(
            seen.iter()
                .all(|(_, auth)| auth.as_deref() == Some("Bearer secret-token"))
        );
    }

    #[test]
    fn single_files_transfer_only_the_file() {
        let mock = Mock::start();
        let github = GitHub::new(mock.api()).with_token(None);
        let spec = spec("github:octo/tools:README.md@main");
        let resolved = github.resolve(&spec).unwrap();
        assert_eq!(resolved.kind, ContentKind::File);
        assert_eq!(fetch(&github, &spec, &resolved), b"hello");
        let urls = mock.urls();
        assert!(urls.iter().any(|u| u.contains("/contents/README.md")));
        assert!(!urls.iter().any(|u| u.contains("/tarball/")));
        assert!(
            mock.seen.lock().unwrap().iter().all(|(_, auth)| auth.is_none()),
            "anonymous without a token"
        );
    }

    #[test]
    fn release_assets_match_exactly_one_asset() {
        let mock = Mock::start();
        let github = GitHub::new(mock.api()).with_token(None);
        let linux = spec("github-release:octo/tools@latest#tools-linux*");
        let resolved = github.resolve(&linux).unwrap();
        assert_eq!(resolved.version, "11");
        assert_eq!(fetch(&github, &linux, &resolved), b"linux build");
        let error = github
            .resolve(&spec("github-release:octo/tools@latest#tools-*"))
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.details["matches"].to_string().contains("tools-mac.tar.gz"));
    }

    #[test]
    fn failures_map_to_stable_codes_without_the_token() {
        let mock = Mock::start();
        let github = GitHub::new(mock.api()).with_token(Some("secret-token".into()));
        for (text, code) in [
            ("github:octo/missing", "not_found"),
            ("github:octo/limited", "unavailable"),
            ("github:octo/broken", "milkpan.fetch_failed"),
        ] {
            let error = github.resolve(&spec(text)).unwrap_err();
            assert_eq!(error.code.as_str(), code, "{text}");
            assert_eq!(error.details["spec"].to_string(), format!("{:?}", text));
            let rendered = format!("{error:?} {}", serde_json::to_string(&error).unwrap());
            assert!(!rendered.contains("secret-token"), "{rendered}");
        }
        assert!(!format!("{github:?}").contains("secret-token"));
        let unreachable = GitHub::new("http://127.0.0.1:9").with_token(None);
        assert_eq!(
            unreachable.resolve(&spec("github:octo/tools")).unwrap_err().code,
            ErrorCode::Unavailable
        );
    }

    #[test]
    #[ignore = "reaches the real GitHub API"]
    fn live_public_repository() {
        let github = GitHub::new(DEFAULT_API);
        let spec = spec("github:octocat/Hello-World:README");
        let resolved = github.resolve(&spec).unwrap();
        assert_eq!(resolved.version.len(), 40);
        assert!(!fetch(&github, &spec, &resolved).is_empty());
    }
}
