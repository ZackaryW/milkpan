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

Without `@ref`, the default branch is used. An asset pattern must match exactly
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
milkpan reclaim                                          # drop expired leases, delete unheld content
milkpan scope list                                       # scopes in the policy
milkpan explain --request request.json                   # explain a policy decision
```

A `<lease>` is a lease id or a spec held by `--scope` (default `default`).
`--principal` defaults to `$MILKPAN_PRINCIPAL`, then `user:<login>`. Add
`--format json` for the protocol envelope. `reacquire --due` exits 2 when some
leases could not be refreshed.

## Configuration

The bucket lives in one directory: `--home`, then `$MILKPAN_HOME`, then the
per-user data directory (for example `~/.local/share/milkpan`).

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

## Credentials

Requests use a token from `GH_TOKEN`, then `GITHUB_TOKEN`, then
`gh auth token`, and are anonymous otherwise. Tokens are never written to
disk, logs, output or error details. `MILKPAN_GITHUB_API` points milkpan at
another API base; `gh` is only asked for a token when talking to
`api.github.com`.

## Bucket layout

```text
<home>/milkpan.toml            optional policy
<home>/state.json              leases and resolution records
<home>/lock                    writers' exclusive lock
<home>/blobs/sha256/<2>/<62>   immutable content, addressed by SHA-256
<home>/trees/<digest>/         snapshots extracted from their blobs
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
- `cli::BucketCommands` registers the bucket commands next to the binary's own.

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
