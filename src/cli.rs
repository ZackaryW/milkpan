//! Bucket commands on panned's CLI contract, reusable by depending binaries.
//!
//! The bucket is built lazily on the first command that needs it, so
//! `protocol` and argument errors work without a home or configuration.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use clap::{Arg, ArgAction, ArgMatches, Command};
use panned::{
    Authorizer, Decision, Error, Principal, Request, Scope, ScopeList,
    cli::{CommandSet, Ctx, Outcome, error_json},
};
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::{Bucket, Lease};

type Build = dyn Fn(&Path) -> Result<Bucket, Error> + Send + Sync;

/// A bucket built on first use from a home directory.
pub struct BucketHandle {
    home: PathBuf,
    build: Box<Build>,
    bucket: OnceLock<Result<Arc<Bucket>, Error>>,
}

impl BucketHandle {
    /// A handle that builds the bucket at `home` with `build` when first needed.
    pub fn new(
        home: impl Into<PathBuf>,
        build: impl Fn(&Path) -> Result<Bucket, Error> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            home: home.into(),
            build: Box::new(build),
            bucket: OnceLock::new(),
        })
    }

    /// The bucket, or the error building it failed with.
    pub fn bucket(&self) -> Result<Arc<Bucket>, Error> {
        self.bucket
            .get_or_init(|| (self.build)(&self.home).map(Arc::new))
            .clone()
    }
}

impl Authorizer for BucketHandle {
    fn authorize(&self, request: &Request) -> Result<Decision, Error> {
        self.bucket()?.authorizer().authorize(request)
    }
}

impl ScopeList for BucketHandle {
    fn scopes(&self) -> Result<Vec<Scope>, Error> {
        self.bucket()?.scopes().scopes()
    }
}

/// The bucket home: `--home`, then `MILKPAN_HOME`, then the per-user data directory.
pub fn resolve_home(args: &[OsString]) -> PathBuf {
    let flag = args
        .windows(2)
        .find(|w| w[0] == "--home")
        .map(|w| PathBuf::from(&w[1]))
        .or_else(|| {
            args.iter()
                .find_map(|a| a.to_str()?.strip_prefix("--home=").map(PathBuf::from))
        });
    flag.or_else(|| {
        std::env::var_os("MILKPAN_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
    .or_else(|| dirs::data_local_dir().map(|d| d.join("milkpan")))
    .unwrap_or_else(|| PathBuf::from(".milkpan"))
}

fn default_principal() -> Principal {
    if let Ok(principal) = std::env::var("MILKPAN_PRINCIPAL")
        && !principal.is_empty()
    {
        return Principal(principal);
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into());
    Principal(format!("user:{user}"))
}

/// `acquire`, `reacquire`, `lease list|show`, `release`, `path` and `reclaim`.
pub struct BucketCommands {
    handle: Arc<BucketHandle>,
}

impl BucketCommands {
    /// Commands over the bucket behind `handle`.
    pub fn new(handle: Arc<BucketHandle>) -> Self {
        Self { handle }
    }
}

fn scope_arg() -> Arg {
    Arg::new("scope")
        .long("scope")
        .default_value("default")
        .help("Scope holding the lease")
}

fn principal_arg() -> Arg {
    Arg::new("principal")
        .long("principal")
        .help("Principal (default: $MILKPAN_PRINCIPAL or user:<login>)")
}

fn lease_arg() -> Arg {
    Arg::new("lease")
        .required(true)
        .help("Lease id, or a spec held by --scope")
}

fn to_json(value: impl serde::Serialize) -> Result<Json, Error> {
    serde_json::to_value(value).map_err(|e| Error::new(panned::ErrorCode::Internal, e.to_string()))
}

struct Args<'a>(&'a ArgMatches);

impl Args<'_> {
    fn text(&self, name: &str) -> Option<&str> {
        self.0.get_one::<String>(name).map(String::as_str)
    }

    fn scope(&self) -> Result<Scope, Error> {
        Scope::new(self.text("scope").unwrap_or("default"))
    }

    fn principal(&self) -> Principal {
        self.text("principal")
            .map_or_else(default_principal, |p| Principal(p.into()))
    }

    fn lease(&self, bucket: &Bucket) -> Result<Lease, Error> {
        let reference = self.text("lease").expect("required");
        match Uuid::parse_str(reference) {
            Ok(id) => bucket.lease(id),
            Err(_) => bucket.find(reference, &self.scope()?),
        }
    }
}

impl CommandSet for BucketCommands {
    fn register(&self, root: Command) -> Command {
        root.arg(
            Arg::new("home")
                .long("home")
                .global(true)
                .help("Bucket home (default: $MILKPAN_HOME)"),
        )
        .subcommand(
            Command::new("acquire")
                .about("Acquire a spec for a scope, fetching it only if new")
                .arg(Arg::new("spec").required(true))
                .arg(scope_arg())
                .arg(principal_arg()),
        )
        .subcommand(
            Command::new("reacquire")
                .about("Resolve a lease again, or every due lease with --due")
                .arg(
                    Arg::new("lease")
                        .required_unless_present("due")
                        .help("Lease id, or a spec held by --scope"),
                )
                .arg(
                    Arg::new("due")
                        .long("due")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("lease"),
                )
                .arg(scope_arg())
                .arg(principal_arg()),
        )
        .subcommand(
            Command::new("lease")
                .about("Inspect leases")
                .subcommand_required(true)
                .subcommand(
                    Command::new("list")
                        .about("List leases")
                        .arg(Arg::new("scope").long("scope").help("Only this scope's leases")),
                )
                .subcommand(
                    Command::new("show")
                        .about("Show one lease")
                        .arg(lease_arg())
                        .arg(scope_arg()),
                ),
        )
        .subcommand(
            Command::new("release")
                .about("Release a lease and reclaim what it held")
                .arg(lease_arg())
                .arg(scope_arg())
                .arg(principal_arg()),
        )
        .subcommand(
            Command::new("path")
                .about("Print the local path of a lease's content, or of a file in a snapshot")
                .arg(lease_arg())
                .arg(Arg::new("file"))
                .arg(scope_arg())
                .arg(principal_arg()),
        )
        .subcommand(Command::new("reclaim").about("Drop expired leases and delete unheld content"))
    }

    fn dispatch(&self, name: &str, matches: &ArgMatches, _ctx: &Ctx) -> Option<Result<Outcome, Error>> {
        if !["acquire", "reacquire", "lease", "release", "path", "reclaim"].contains(&name) {
            return None;
        }
        Some(
            self.handle
                .bucket()
                .and_then(|bucket| run(&bucket, name, matches)),
        )
    }
}

fn complete(value: impl serde::Serialize) -> Result<Outcome, Error> {
    to_json(value).map(Outcome::Complete)
}

fn run(bucket: &Bucket, name: &str, matches: &ArgMatches) -> Result<Outcome, Error> {
    let args = Args(matches);
    match name {
        "acquire" => complete(bucket.acquire(
            args.text("spec").expect("required"),
            &args.scope()?,
            &args.principal(),
        )?),
        "reacquire" if matches.get_flag("due") => {
            let (mut reacquired, mut failed) = (Vec::new(), Vec::new());
            for (id, outcome) in bucket.reacquire_due(&args.principal())? {
                match outcome {
                    Ok(acquired) => reacquired.push(to_json(acquired)?),
                    Err(error) => failed.push(json!({ "lease": id, "error": error_json(&error) })),
                }
            }
            let partial = !failed.is_empty();
            let result = json!({ "reacquired": reacquired, "failed": failed });
            Ok(if partial {
                Outcome::NonComplete(result)
            } else {
                Outcome::Complete(result)
            })
        }
        "reacquire" => complete(bucket.reacquire(args.lease(bucket)?.id, &args.principal())?),
        "lease" => match matches.subcommand() {
            Some(("list", sub)) => {
                let scope = Args(sub).text("scope").map(Scope::new).transpose()?;
                complete(json!({ "leases": bucket.leases(scope.as_ref())? }))
            }
            Some(("show", sub)) => complete(Args(sub).lease(bucket)?),
            _ => unreachable!("subcommand required"),
        },
        "release" => {
            complete(json!({ "released": bucket.release(args.lease(bucket)?.id, &args.principal())? }))
        }
        "path" => {
            let lease = args.lease(bucket)?;
            complete(json!({ "path": bucket.path(lease.id, args.text("file"), &args.principal())? }))
        }
        "reclaim" => complete(bucket.reclaim()?),
        _ => unreachable!("filtered in dispatch"),
    }
}
