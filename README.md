# milkpan

A small resource bucket for GitHub. Milkpan acquires repository snapshots,
single files and release assets at exact versions, stores them
content-addressed, leases them to scopes and reacquires them when their
selector moves. Downloads happen only for versions it has not stored yet.

Milkpan is built on [panned](https://github.com/ZackaryW/panned): every lease operation is a panned
authorization request, and the binary speaks panned's CLI protocol (JSON
envelope, error codes, exit codes, `--protocol`).

## Install

Download the archive for your platform from
[Releases](https://github.com/ZackaryW/milkpan/releases) and check it against
`SHA256SUMS`, or build from source with `cargo install --git https://github.com/ZackaryW/milkpan`.

## Specs

| Spec | Fetches | Version |
| --- | --- | --- |
| `github:owner/repo[@ref]` | the repository's files at the commit, without history | commit SHA |
| `github:owner/repo:path/to/file[@ref]` | only that file | commit SHA |
| `github-release:owner/repo@tag#asset` | one release asset; `@latest` and `*` patterns allowed | asset id |
| `local:/abs/path/to/repo[@ref]` | a local git repository's committed files at the commit (needs `git`) | commit SHA |

Release assets named `*.tar.gz`, `*.tgz` or `*.zip` are extracted into a
directory (a top-level folder shared by every entry is stripped, and
executable bits are kept on Unix); other assets stay single files.

Without `@ref`, the default branch is used (`HEAD` for local repositories). An asset pattern must match exactly
one asset. Owner and repository names are case-insensitive; paths, refs and
asset names are not.

## Commands

```sh
milkpan acquire github:octo/tools@v1.0 --scope ci        # resolve, fetch if new, lease
milkpan path <lease> docs/guide.md                       # local path of a file in a snapshot
milkpan reacquire <lease>                                # resolve again, swap if it moved
milkpan reacquire --due                                  # every lease past its refresh cadence
milkpan lease list [--scope ci]
milkpan lease show <lease>
milkpan release <lease>                                  # drop the lease, reclaim its content
milkpan link <lease> --name tool --file tool             # stable link plus <home>/bin/tool
milkpan link list
milkpan unlink tool
milkpan slot list                                        # links as slots with retained versions
milkpan slot switch tool <version>                       # roll back; effective on the next run
milkpan reclaim                                          # drop expired leases, delete unheld content
milkpan scope list                                       # scopes in the policy
milkpan explain --request request.json                   # explain a policy decision
```

A `<lease>` is a lease id or a spec held by `--scope` (default `default`).
`--principal` defaults to `$MILKPAN_PRINCIPAL`, then `user:<login>`. Add
`--format json` for the protocol envelope. `reacquire --due` exits 2 when some
leases could not be refreshed.

## Configuration

The bucket lives in one directory: `--home`, then `$MILKPAN_HOME`, then
`~/.milkpan` (`%USERPROFILE%\.milkpan` on Windows). Other tools built on milkpan,
such as Powerspec, use the same default, so they share one bucket and its
stored content.

Before 0.3.0 the default was the per-user data directory
(`~/.local/share/milkpan`, `%LOCALAPPDATA%\milkpan`). That bucket is not read
anymore: move it to `~/.milkpan` while no milkpan is running, or keep using it
with `MILKPAN_HOME=<old path>`.

Without a configuration file, every operation is allowed in scope `default`
and nowhere else. `milkpan.toml` in the home replaces that with panned
`[[rule]]` tables. Lease settings are `lease` obligations on the same rules:

```toml
# Scope ci may use anything from the octo organization.
[[rule]]
scope = "ci"
effect = "allow"
actions = ["acquire", "reacquire", "read", "release"]
resources = ["github:octo/*", "github-release:octo/*"]
obligations = [{ name = "lease", params = { ttl = "1d", refresh = "10m", retain = "1" } }]

# Fast-moving tools refresh more often; this merges with the rule above.
[[rule]]
scope = "ci"
effect = "allow"
actions = ["acquire"]
resources = ["github:octo/fast-*"]
obligations = [{ name = "verify" }]

# Never fetch archived repositories.
[[rule]]
scope = "ci"
effect = "deny"
actions = ["acquire"]
resources = ["github:octo/archived-*"]
```

| `lease` parameter | Default | Meaning |
| --- | --- | --- |
| `ttl` | `7d` | time from the last renewal until the lease expires |
| `refresh` | `1h` | how long a resolution stays fresh; acquiring within it needs no network |
| `retain` | `0` | previous versions kept from reclamation |

Durations take `s`, `m`, `h` or `d`. When several matching rules set the same
parameter to different values, the operation fails with
`milkpan.conflicting_obligation` instead of picking one. Every obligation must
be handled by a hook: `lease` and `verify` (re-hash the stored content) are
built in; any other obligation fails with `milkpan.unmet_obligation`.

Policies see `source.provider` (`github`, `github-release`) and `source.kind`
(`snapshot`, `file`, `release`) as context facts.

## Links, shims and updates on the next run

`milkpan link <lease> --name tool` keeps `<home>/links/tool` pointing at the
lease's current content: a relative symlink on Unix, a directory junction on
Windows. Every reacquisition that changes the version repoints it without
touching the old content. With `--file <path>`, milkpan also writes
`<home>/bin/tool`: a symlink on Unix, and `tool.cmd` plus a POSIX script on
Windows. It launches that file through the link. Put `<home>/bin` on PATH
once.

Because shims go through the link, an update never replaces a running
program: the program keeps the version it started with, and the next start
runs the new one (the way Scoop updates apps). An old version that is still
running when reclamation runs is reported as `in_use` and deleted on a later
reclamation.

Linked leases do not expire; `unlink` or `release` lets their content go.
Links are panned slots: `slot list` shows each link's current version and
the versions retained by `lease(retain=N)`, and `slot switch` rolls a link
back to one of them. The spec's selector is unchanged, so the next due
reacquisition moves it forward again; pin a tag in the spec to stay put.

Milkpan manages itself the same way:

```sh
milkpan acquire "github-release:ZackaryW/milkpan@latest#milkpan-*-x86_64-pc-windows-msvc.zip" --scope self
milkpan link github-release:zackaryw/milkpan@latest#milkpan-*-x86_64-pc-windows-msvc.zip --scope self --name milkpan --file milkpan.exe
milkpan reacquire --due      # later: the next start of <home>/bin/milkpan runs the new release
```

This needs a policy allowing scope `self` (the built-in policy allows only
`default`). On Windows, `Ctrl+C` in `cmd` asks "Terminate batch job?" because
the launcher is a `.cmd` file.

## Credentials

Requests use a token from `GH_TOKEN`, then `GITHUB_TOKEN`, then
`gh auth token`, and are anonymous otherwise. Tokens are never written to
disk, logs, output or error details. `MILKPAN_GITHUB_API` points milkpan at
another API base; `gh` is only asked for a token when talking to
`api.github.com`.

## Bucket layout

By default `<home>` is `~/.milkpan`.

```text
<home>/milkpan.toml            optional policy
<home>/state.json              leases and resolution records
<home>/lock                    writers' exclusive lock
<home>/blobs/sha256/<2>/<62>   immutable content, addressed by SHA-256
<home>/trees/<digest>/         archives extracted from their blobs
<home>/files/<digest>/<name>   named views of single-file blobs, for links
<home>/links/<name>            stable links to lease content
<home>/bin/<name>              launchers through links
<home>/tmp/                    staging for downloads and extraction
```

Content is staged and renamed into place, and state is replaced atomically, so
an interrupted command leaves the previous state. Downloads happen outside the
lock; only state changes hold it.

## Extending milkpan

Depending crates (such as Saucepan) build their own bucket and binary:

- `Provider` adds a spec prefix: resolve a spec to an exact version, fetch its content.
- `BucketBuilder::authorizer` replaces the policy with any panned `Authorizer`.
- `AfterFetch` hooks fulfil obligations; `BeforeReclaim` hooks can keep blobs.
- `cli::BucketCommands` registers the bucket commands next to the binary's own;
  `cli::BucketHandle` also implements panned's `Slot` for `GenericCommands::with_slots`.

```rust,no_run
use milkpan::{BucketBuilder, cli::{BucketCommands, BucketHandle}, provider::github::GitHub};
use panned::cli::{GenericCommands, Runner};

let handle = BucketHandle::new("/var/lib/tool", |home| {
    BucketBuilder::new(home).provider(GitHub::from_env()).build()
});
Runner::new("tool", "0.1.0")
    .with(GenericCommands::new(handle.clone(), handle.clone()))
    .with(BucketCommands::new(handle))
    .main()
```

## Developing with a local panned

`Cargo.lock` pins the panned commit; `cargo update -p panned` moves it to
panned's latest `main`. To build against a local checkout instead, without
editing tracked files:

```sh
cargo test --config 'patch."https://github.com/ZackaryW/panned".panned.path="../panned"'
```

The patch also rewrites `Cargo.lock` to the local path; restore it with
`git checkout Cargo.lock` before committing.

## License

MIT
