<div align="center">

# ddev-workspaces

**Create isolated Git worktrees and prepare each one as a safe, reproducible DDEV workspace.**

[![CI](https://github.com/alessandrotesoro/ddev-workspaces/actions/workflows/ci.yml/badge.svg)](https://github.com/alessandrotesoro/ddev-workspaces/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/alessandrotesoro/ddev-workspaces?style=flat-square)](https://github.com/alessandrotesoro/ddev-workspaces/releases/latest)
[![Rust](https://img.shields.io/badge/Rust-1.97.1-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)

[Features](#features) • [Installation](#installation) • [Quick start](#quick-start) • [Configuration](#configuration) • [Command reference](#command-reference)

</div>

`ddev-workspaces` is a local CLI for creating and managing project workspaces backed by [Git worktrees](https://git-scm.com/docs/git-worktree). It can copy ignored local files, run explicit preparation commands, check runtime readiness, and bind each workspace to an isolated [DDEV](https://ddev.com/) project.

The tool is intentionally conservative: Git remains the authority for source state, DDEV remains the authority for runtime state, and destructive operations fail closed unless ownership and confirmation can be proven.

## Features

- Creates a new branch and worktree from an explicit commit or the commit advertised by `origin`'s default branch.
- Supports source-only workspaces when DDEV or runtime preparation is unnecessary.
- Copies tracked templates or explicitly named local files without overwriting destinations.
- Runs commands as argument arrays without invoking a shell.
- Verifies declared files, environment keys, Git integrity, and DDEV readiness.
- Records exact ownership and creation provenance before mutating Git or DDEV state.
- Provides read-only creation and removal preflights with `--dry-run`.
- Retains branches and DDEV data by default when removing a workspace.

## Installation

Prebuilt releases target Apple Silicon macOS and x86-64 Linux.

### Homebrew

```sh
brew install alessandrotesoro/tap/ddev-workspaces
```

### Shell installer

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/alessandrotesoro/ddev-workspaces/releases/latest/download/ddev-workspaces-installer.sh | sh
```

### npm

```sh
npm install --global @sematico/ddev-workspaces
```

### Build from source

This method requires the Rust toolchain defined in [`rust-toolchain.toml`](rust-toolchain.toml):

```sh
cargo install --git https://github.com/alessandrotesoro/ddev-workspaces \
  --tag v0.1.0 \
  --locked
```

> [!NOTE]
> Git is always required at runtime. DDEV is required only for repositories that configure a `[ddev]` section.

## Quick start

From the main worktree of the repository you want to manage:

1. Add the worktree directory to `.gitignore`:

   ```sh
   printf '%s\n' '.worktrees/' >> .gitignore
   ```

2. Add `.ddev-workspaces.toml` at the repository root:

   ```toml
   version = 1
   project_id = "example-app"
   workspace_root = ".worktrees"

   [ddev]
   app_root = "."
   ```

3. Diagnose the repository and preview creation:

   ```sh
   ddev-workspaces doctor
   ddev-workspaces create --dry-run feature-name
   ```

4. Create and inspect the workspace:

   ```sh
   ddev-workspaces create feature-name
   ddev-workspaces list
   ```

5. Preview removal, then confirm it with the exact workspace name:

   ```sh
   ddev-workspaces remove --dry-run feature-name
   ddev-workspaces remove --confirm feature-name feature-name
   ```

> [!IMPORTANT]
> Without `--base`, creation uses the exact commit advertised by `origin`'s symbolic `HEAD`. That commit must already exist locally. The tool never fetches automatically; fetch the remote default branch yourself when instructed.

### Source-only workspaces

Use `--source-only` to create the Git worktree while skipping file preparation, declared commands, readiness checks, and DDEV:

```sh
ddev-workspaces create --source-only docs-update
```

## Configuration

Configuration is read from a strict `.ddev-workspaces.toml` file in the main worktree. Unknown fields and unsupported versions are rejected. The `version` field identifies the configuration format; it is independent of the installed `ddev-workspaces` version.

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

[[files]]
label = "DDEV configuration"
destination = ".ddev/config.yaml"
source_env = "DDEV_WORKSPACE_CONFIG"

[[commands]]
label = "Install dependencies"
cwd = "."
argv = ["ddev", "composer", "install"]
sensitive = false

[[checks]]
label = "Application key"
kind = "env-key"
path = ".env"
key = "APP_KEY"

[[checks]]
label = "Installed dependencies"
kind = "path-exists"
path = "vendor/autoload.php"
```

| Setting | Purpose |
| --- | --- |
| `version` | Configuration format version. Must be `1`; this is not the CLI release version. |
| `project_id` | Stable DNS-safe project identifier used in DDEV names. |
| `workspace_root` | Repository-relative, already ignored directory for worktrees. |
| `[ddev].app_root` | Optional repository-relative directory containing `.ddev/config.yaml`. |
| `[[files]]` | Copies one tracked `template` or one absolute regular file named by `source_env`. Destinations must be ignored and are never overwritten. |
| `[[commands]]` | Runs an explicit `argv` in `cwd`, in declaration order and without a shell. Set `sensitive = true` to suppress the command and its output. |
| `[[checks]]` | Requires either a path (`path-exists`) or a non-empty environment key (`env-key`). |

When DDEV is enabled, the generated project name is deterministic: `dw-<project_id>--<workspace-name>`. The tool writes only the ignored `.ddev/config.ddev-workspaces.yaml` override before starting DDEV.

## Command reference

| Command | Description |
| --- | --- |
| `doctor [PATH]` | Diagnoses a repository or managed workspace without modifying it. |
| `create [--base REV] [--source-only] [--dry-run] NAME` | Creates an isolated managed workspace. |
| `list` | Lists owned workspaces for the current repository and recomputes their readiness. |
| `remove [--dry-run] [--delete-ddev-data] [--confirm NAME] [--confirm-data NAME] NAME` | Removes a proven owned worktree while retaining its branch. |

Run `ddev-workspaces <command> --help` for complete option details.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The requested operation completed or the inspected state is ready. |
| `1` | A diagnostic, preflight, operation, or safety check is not ready. |
| `2` | Invalid command-line usage. |

## Safety model

`ddev-workspaces` is designed to stop instead of guessing:

- It never fetches, pushes, adopts an existing branch, resets a branch, or silently prunes Git metadata.
- It reserves an ownership record before creating a worktree and preserves failed creations for diagnosis.
- It refuses symlink escapes, unmanaged paths, dirty worktrees, invalid ownership records, and mismatched DDEV identities.
- It never overwrites a prepared file and requires generated destinations to be ignored by Git.
- Normal removal stops and unlists the exact DDEV project, removes the worktree without force, retains its branch, and retains DDEV data.
- DDEV data removal requires both `--delete-ddev-data` and a second exact `--confirm-data NAME` confirmation.

> [!WARNING]
> A failed creation may intentionally leave its ownership record, branch, and worktree in place. Read the reported error, inspect the workspace with `doctor PATH`, correct the underlying problem, and only then retry or remove it.

## Troubleshooting

### The remote default commit is missing locally

Fetch the default branch named in the error, then retry:

```sh
git fetch origin main
```

### The workspace root or prepared file is not ignored

Add the reported path to `.gitignore` or `.git/info/exclude`. The tool never edits ignore rules for you.

### Creation failed after ownership was reserved

Do not delete the worktree or ownership record blindly. Run `ddev-workspaces doctor PATH` using the path printed in the failure report, fix the named source, command, check, or DDEV issue, and inspect it again.

### Removal requires confirmation

Pass the workspace name twice so the option and positional argument match exactly:

```sh
ddev-workspaces remove --confirm feature-name feature-name
```

## Development

The project pins its Rust toolchain and dependency lockfile. Run the same core checks used by CI:

```sh
cargo fmt --all --check
cargo check --all-targets --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings -D clippy::perf
cargo test --all-targets --all-features --locked
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --all-features --locked --document-private-items
cargo deny check
```

## License

Licensed under the [MIT License](LICENSE).
