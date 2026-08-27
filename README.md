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
project_id = "filebean"
workspace_root = ".worktrees"

[ddev]
app_root = "."

[[files]]
label = "Laravel environment"
destination = "apps/laravel/.env"
template = "apps/laravel/.env.example"

[[commands]]
label = "Install PHP dependencies"
cwd = "apps/laravel"
argv = ["ddev", "composer", "install"]

[[checks]]
label = "Laravel app key"
kind = "env-key"
path = "apps/laravel/.env"
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

`create NAME` first resolves the base and checks all safe collisions. With no
`--base`, it queries `origin`'s advertised symbolic `HEAD` and requires that
full commit to be present locally. It never fetches, pushes, adopts an
existing branch, or uses the current `HEAD` as a fallback. The ownership
record is reserved before the Git worktree is created. Once reserved, any
failure preserves the record and worktree for diagnosis.

`create --source-only NAME` performs Git worktree, tracked-source, submodule,
and local LFS preparation, then skips files, commands, readiness checks, and
DDEV. `--dry-run` performs read-only preflight and prints planned mutations;
it creates no branch, path, file, ownership record, or DDEV state.

`list` reads only ownership records under the current repository's Git common
directory and recomputes compact source/runtime status. It does not scan
other repositories. It returns success when every entry is ready or
source-only, and returns `1` when an entry is invalid or not ready.

`remove NAME` requires a valid tool-owned record, the exact canonical path and
Git worktree, a clean and unlocked worktree, and an exact DDEV identity when
one is present. In a non-interactive terminal, pass `--confirm NAME`. Default
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

Filebean and barn2site are manual acceptance pilots, not automated test
fixtures. Before either pilot, obtain separate approval for an untracked
`.ddev-workspaces.toml`, choose an ignored disposable workspace root, and
confirm the intended DDEV name and canonical app root are unused.

For Filebean, verify the advertised default-branch SHA with `doctor` and a
dry-run, name only the tracked Laravel environment template, and let the
existing project setup command own Composer, app-key, migration, and frontend
work. Confirm the generated environment, dependencies, build manifest, exact
DDEV identity, running status, and Mutagen health before exercising removal.

For barn2site, run `doctor` first and confirm its hidden index flags, missing
tracked path, nested gitlink, private submodule, and prunable metadata are
visible. Use `--source-only` while validating recursive submodule readiness;
configure only the cwd-specific commands and local sources explicitly chosen
for the pilot. Do not copy `.ddev`, uploads, certificates, snapshots,
databases, or other hidden files in bulk, and do not add upload or database
commands without separate approval.
