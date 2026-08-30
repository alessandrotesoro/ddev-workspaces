# ddev-workspaces

`ddev-workspaces` is a small local CLI for creating Git worktrees that can be
prepared and bound to an isolated DDEV project. It has four commands:

```text
ddev-workspaces doctor [PATH]
ddev-workspaces create [--base REV] [--source-only] [--dry-run] NAME
ddev-workspaces list
ddev-workspaces remove [--dry-run] [--delete-ddev-data] [--confirm NAME] [--confirm-data NAME] NAME
```

The binary is intentionally conservative. Git remains the authority for
repository and worktree state, DDEV remains the authority for runtime state,
and the project configuration is a finite, strict TOML file.

## Release planning

- [v0.1.0 macOS ARM release plan](docs/plans/2026-08-29-1932-feat-v0-1-0-macos-arm-release-plan.md)
- [Permanent project configuration rollout plan](docs/plans/2026-08-29-2232-feat-permanent-project-config-rollout-plan.md)

## Build and install locally

The repository pins the development toolchain in `rust-toolchain.toml`.

```bash
cargo build --release
cargo install --path .
```

The runtime dependencies are the installed `git` command and, when a project
declares `[ddev]`, the installed `ddev` command. Automated tests use temporary
Git repositories and fake commands; they do not need Docker, DDEV, network
access, or project credentials.

## Configure a repository

Create `.ddev-workspaces.toml` at the repository root. The file must contain
`version = 1`, a DNS-safe `project_id`, and an already-ignored
`workspace_root`. Add the workspace root to a local Git exclude or an existing
ignore rule yourself; the tool never edits ignore files.

```toml
version = 1
project_id = "example-app"
workspace_root = ".worktrees"

[ddev]
app_root = "."

[[files]]
label = "Local environment"
destination = ".env"
template = ".env.example"

[[commands]]
label = "Install dependencies"
cwd = "."
argv = ["ddev", "composer", "install"]

[[checks]]
label = "Application key"
kind = "env-key"
path = ".env"
key = "APP_KEY"
```

Configuration is strict: unknown fields, future versions, escaping paths,
invalid names, unsupported checks, and file rules that do not select exactly
one source are rejected. File sources are either tracked worktree templates or
absolute regular files named by a `source_env` variable. Destination files are
never overwritten. Commands are ordered argument arrays and run without a
shell. `env-key` only checks whether a named key has a non-empty value; it does
not print the value.

When `[ddev]` is present, the app root must contain a named `.ddev/config.yaml`
after file preparation. The tool creates only the ignored
`.ddev/config.ddev-workspaces.yaml` override, containing the deterministic
name `dw-<project_id>--<workspace-name>`.

## Commands and readiness

`doctor` is read-only. It reports Git integrity, hidden index flags, missing
tracked paths, sparse checkout, submodule/LFS state, worktree metadata, config
validity, and DDEV observations. Warnings about unrelated stale or prunable
state are reported with manual guidance; they are not repaired automatically.
DDEV discovery runs against an ephemeral copy of its registry and global
configuration so DDEV cannot prune the user's registry during diagnosis.

`create NAME` first resolves the base and checks all safe collisions. With no
`--base`, it queries `origin`'s advertised symbolic `HEAD` and requires that
full commit to be present locally. It never fetches, pushes, adopts an
existing branch, or uses the current `HEAD` as a fallback. The ownership
record is reserved before the Git worktree is created. It records immutable
creation mode and the original DDEV app root, when any, so later status and
cleanup do not guess from mutable configuration. Once reserved, any failure
preserves the record and worktree for diagnosis.

`create --source-only NAME` performs Git worktree, tracked-source, submodule,
and local LFS preparation, then skips files, commands, readiness checks, and
DDEV. `--dry-run` performs read-only preflight and prints planned mutations;
it creates no branch, path, file, ownership record, or DDEV state.

`list` reads only ownership records under the current repository's Git common
directory and recomputes compact source/runtime status. It does not scan
other repositories. It returns success when every entry is ready or
explicitly source-only, and returns `1` when an entry is invalid or not
ready. A failed full creation never becomes source-only merely because its
runtime prerequisites are incomplete.

`remove NAME` requires a valid tool-owned record, the exact canonical path and
Git worktree, a clean and unlocked worktree, and the exact originally recorded
DDEV identity when one is present, even if current configuration changed. In a
non-interactive terminal, pass `--confirm NAME`. Default
cleanup issues only `ddev stop --unlist <exact-name>`, removes the worktree
without force, retains the branch and DDEV data, and removes the ownership
record last. `--delete-ddev-data` additionally requires
`--confirm-data NAME` and uses `ddev stop --remove-data --unlist`; it never
invokes `ddev delete`.

Exit code `0` means the requested result is ready or completed, `1` means a
diagnostic, preflight, operation, or safety check is not ready, and `2` is
invalid CLI usage.

## Failure recovery

After a post-reservation failure, use the printed record path and run
`doctor PATH` on the preserved worktree. Fix the named issue manually—such as
a missing local secret source, submodule credentials, a declared command, or
DDEV state—then run `list` or `doctor` again. The tool does not silently retry,
roll back across Git and DDEV, delete branches, prune metadata, or remove
unrelated resources.

## Pilot acceptance

Documentably and Posts Table Pro are the two manual acceptance pilots; neither
is an automated fixture or a persisted project configuration. Both run from
fresh disposable clones at the remote-advertised default commit, with an
untracked `.ddev-workspaces.toml`, clone-local ignore rules, unique workspace
identities, and exact cleanup after verification.

Documentably is source-only coverage. `doctor`, dry-run, `create
--source-only`, ownership/source verification, `list`, and safe removal prove
the Git lifecycle and tracked dotfiles. Because source-only creation
intentionally skips runtime commands and checks, the repository's pinned
`corepack pnpm install --frozen-lockfile` and `corepack pnpm build` run
manually inside the owned disposable worktree; `list` then verifies the
ignored `packages/cli/dist/index.js` artifact. This pilot does not test DDEV
or declared-command orchestration.

Posts Table Pro is disposable DDEV coverage around tracked WordPress-plugin
source, not a functional WordPress/database test. One explicit local file rule
publishes a temporary ignored `.ddev/config.yaml`; the configuration disables
settings management and Mutagen and omits the database. No uploads,
certificates, snapshots, secrets, imports, or package commands are supplied.
Acceptance requires the exact unique DDEV name/path/running identity and then
confirmed removal of the owned worktree, registration, container, network,
temporary config, clone, and any exact unreferenced project-built image.

### Acceptance evidence

The replacement pilots passed on 2026-08-29 with the release binary built from
the PR source at `b53ddc3a408dcbe4412d3a361ad39aadef5cc33f`.
After the final read-only DDEV-registry correction, the disposable Posts Table
Pro runtime pilot was repeated against the corrected release build with the
new identity `dw-ptp-final--head-proof-20260829`; full create, list, managed
doctor, removal dry-run, confirmed removal, registry/container hash controls,
and exact resource cleanup all passed again.

Documentably's remote advertised `refs/heads/main` at
`0bc83fc1f5f410de2a2e45d503152078ca32beed`. In a fresh disposable clone,
`doctor`, source-only dry-run, and `create --source-only dw-doc-acceptance`
passed at that exact SHA. The managed worktree was clean and retained the
tracked `.agents`, `.codex`, `.github`, `.gitignore`, and `.mcp.json`
tree/blob identities. `corepack pnpm install --frozen-lockfile` and `corepack
pnpm build` passed with the repository-pinned pnpm `10.34.5`; `list` and
managed-workspace `doctor` then reported the ignored
`packages/cli/dist/index.js` artifact ready. Removal dry-run and `remove
--confirm dw-doc-acceptance dw-doc-acceptance` passed. The exact retained
pilot branch and disposable clone were then removed; the normal checkout
remained clean at the same commit.

Posts Table Pro's remote advertised `refs/heads/master` at
`6ed8023b236ac3819060760036dfc5c45e19359c`. In a fresh disposable clone
outside the parent DDEV application, doctor and full-create dry-run passed,
then `create acceptance-20260829` reached `READY` as
`dw-ptp-pilot--acceptance-20260829` at the exact canonical workspace path.
The copied `.ddev/config.yaml` was private and byte-identical to its named
temporary source. DDEV reported one healthy WordPress-classified web runtime,
Mutagen disabled, and no database container; no project volume, uploads path,
snapshot, secret, import, certificate, or package command was present. `list`,
managed-workspace `doctor`, removal dry-run, and `remove --confirm
acceptance-20260829 acceptance-20260829` passed. The exact retained pilot
branch, project image, isolated DDEV home, temporary config, and disposable
clone were removed. The normal DDEV registry, pre-existing container set, and
shared DDEV network identity matched their pre-pilot hashes.
