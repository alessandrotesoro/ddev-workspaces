<div align="center">

# ddev-workspaces

**Create isolated Git worktrees with safe, reproducible DDEV environments.**

[![CI](https://github.com/alessandrotesoro/ddev-workspaces/actions/workflows/ci.yml/badge.svg)](https://github.com/alessandrotesoro/ddev-workspaces/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/alessandrotesoro/ddev-workspaces?style=flat-square)](https://github.com/alessandrotesoro/ddev-workspaces/releases/latest)
[![Rust](https://img.shields.io/badge/Rust-1.97.1-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)

[Install](#installation) • [Quick start](#quick-start) • [Configure](#configuration) • [Commands](#command-reference) • [Agent plugins](#agent-plugins)

</div>

`ddev-workspaces` is a conservative local CLI for creating and managing project workspaces backed by [Git worktrees](https://git-scm.com/docs/git-worktree). Each workspace can receive ignored local files, run explicit preparation commands, pass readiness checks, and start as an isolated [DDEV](https://ddev.com/) project.

Git remains the authority for source state, DDEV remains the authority for runtime state, and destructive operations stop unless ownership can be proven.

## Features

- Create a branch and worktree from an explicit commit or the commit advertised by the remote default branch.
- Prepare workspaces with tracked templates, named local files, commands, and readiness checks.
- Run source-only workspaces when no runtime is needed.
- Clone a containing DDEV site for nested repositories such as WordPress plugins.
- Record exact ownership before mutating Git or DDEV state.
- Preview creation and removal with `--dry-run`.
- Retain branches and DDEV data by default during removal.

## Installation

Prebuilt releases support Apple Silicon macOS and x86-64 Linux.

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

```sh
cargo install --git https://github.com/alessandrotesoro/ddev-workspaces \
  --tag v0.1.0 \
  --locked
```

> [!NOTE]
> Git is always required. DDEV is required only when the repository configures a `[ddev]` section.

## Quick start

From the repository's main worktree:

1. Ignore the directory that will contain worktrees:

   ```gitignore
   .worktrees/
   ```

2. Create `.ddev-workspaces.toml`:

   ```toml
   version = 1
   project_id = "example-app"
   workspace_root = ".worktrees"

   [ddev]
   app_root = "."
   ```

3. Diagnose, preview, and create the workspace:

   ```sh
   ddev-workspaces doctor
   ddev-workspaces create --dry-run feature-name
   ddev-workspaces create feature-name
   ddev-workspaces list
   ```

4. Preview removal, then confirm the prompt:

   ```sh
   ddev-workspaces remove --dry-run feature-name
   ddev-workspaces remove feature-name
   ```

> [!IMPORTANT]
> Without `--base`, creation uses the exact commit advertised by `origin`'s symbolic `HEAD`. The commit must already exist locally; the CLI never fetches automatically.

### Source-only workspace

Skip file preparation, commands, readiness checks, and DDEV when only a Git worktree is needed:

```sh
ddev-workspaces create --source-only docs-update
```

## Configuration

The CLI reads a strict `.ddev-workspaces.toml` from the main worktree. Unknown fields and unsupported configuration versions are rejected.

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
label = "Installed dependencies"
kind = "path-exists"
path = "vendor/autoload.php"
```

| Setting | Purpose |
| --- | --- |
| `version` | Configuration format version. Must be `1`; it is not the CLI release version. |
| `project_id` | Stable DNS-safe identifier used in generated DDEV names. |
| `workspace_root` | Repository-relative, Git-ignored worktree directory. |
| `[ddev].app_root` | Repository-relative directory containing `.ddev/config.yaml`. |
| `[ddev.source_site]` | Auto-discover a containing DDEV site and generate isolated copies under `~/.ddev-workspaces/sites`. |
| `[[files]]` | Copy one tracked `template` or one absolute regular file named by `source_env`. |
| `[[commands]]` | Run an explicit argument array in `cwd`, without a shell. |
| `[[checks]]` | Require a path or a non-empty environment variable. |

Generated DDEV names are deterministic: `dw-<project_id>--<workspace-name>`.

### Repository nested inside another DDEV site

Source-site mode supports a Git repository contained within a larger DDEV application, such as a WordPress plugin:

```toml
version = 1
project_id = "woocommerce-product-filters"
workspace_root = ".worktrees"

[ddev]
app_root = "."

[ddev.source_site]
repository_path = "wp-content/plugins/_woocommerce-product-filters"
clone_database = true
```

The CLI finds the nearest parent containing a regular `.ddev/config.yaml`, creates `~/.ddev-workspaces/sites` when needed, copies the containing site without Git metadata, `node_modules`, or generated DDEV state, and mounts the new worktree at `repository_path`. No setup environment variables are required. Relative symlinks are preserved only when their targets remain inside the copied source tree; absolute and escaping links are rejected.

When `clone_database = true`, the database passes through a private temporary dump. A source project that was stopped before cloning is restored to its stopped state.

> [!WARNING]
> Run DDEV commands from the generated application root reported by `create` or `list`, not from the nested repository worktree. A bare DDEV command from the worktree may select the original source site.

Removing a source-site workspace recursively deletes its generated application directory. The source site is never deleted.

## Agent plugins

An optional plugin teaches Codex and Claude Code how to configure repositories and operate the CLI safely. Install the CLI first.

### Codex

```sh
codex plugin marketplace add alessandrotesoro/ddev-workspaces
codex plugin add ddev-workspaces@ddev-workspaces
```

### Claude Code

```text
/plugin marketplace add alessandrotesoro/ddev-workspaces
/plugin install ddev-workspaces@ddev-workspaces
```

## Command reference

| Command | Description |
| --- | --- |
| `doctor [PATH]` | Diagnose a repository or managed workspace without modifying it. |
| `create [--base REV] [--source-only] [--dry-run] NAME` | Create an isolated managed workspace. |
| `list` | List managed workspaces and recompute readiness. |
| `remove [--dry-run] [--delete-ddev-data] [--yes] NAME` | Remove a proven workspace while retaining its branch. |

Exit code `0` means ready or complete, `1` means not ready, and `2` indicates invalid command usage.

## Safety model

- The CLI never fetches, pushes, adopts an existing branch, resets a branch, or silently prunes Git metadata.
- Ownership is reserved before creating a worktree.
- Symlink escapes, unmanaged paths, dirty worktrees, invalid records, and mismatched DDEV identities are rejected.
- Prepared files are never overwritten and generated destinations must be ignored by Git.
- Normal removal retains the branch and DDEV data.
- DDEV data deletion requires `--delete-ddev-data` and confirmation through the prompt or `--yes`.

> [!WARNING]
> A failed creation can intentionally retain its ownership record, branch, and worktree for diagnosis. Run `ddev-workspaces doctor PATH`, correct the reported problem, then retry or remove the workspace through the CLI.

## Troubleshooting

<details>
<summary><strong>The remote default commit is missing locally</strong></summary>

Fetch the default branch named in the error, then retry:

```sh
git fetch origin main
```

</details>

<details>
<summary><strong>The workspace root or prepared file is not ignored</strong></summary>

Add the reported path to `.gitignore` or `.git/info/exclude`. The CLI never edits ignore rules automatically.

</details>

<details>
<summary><strong>Creation failed after ownership was reserved</strong></summary>

Run `ddev-workspaces doctor PATH` with the path from the failure report. Fix the reported source, command, check, or DDEV problem instead of deleting state manually.

</details>

<details>
<summary><strong>Removal requires confirmation</strong></summary>

Run the command in a terminal and answer its single prompt. In scripts or other non-interactive environments, pass `--yes`:

```sh
ddev-workspaces remove --yes feature-name
```

</details>

## Development

The project pins its Rust toolchain and lockfile. Run the same core checks as CI:

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
