---
name: ddev-workspaces
description: Create, inspect, diagnose, and safely remove local Git worktrees managed by the ddev-workspaces CLI. Use when a task calls for an isolated DDEV workspace or refers to .ddev-workspaces.toml, managed workspaces, or ddev-workspaces commands.
---

# ddev-workspaces

Use `ddev-workspaces` to manage isolated Git worktrees and their DDEV environments. Let the CLI enforce ownership and safety; do not replace its operations with raw `git worktree`, filesystem deletion, or manual DDEV cleanup.

## Before operating

- Confirm `ddev-workspaces` is installed. If it is missing, direct the user to the project's installation instructions.
- Work from the repository's main worktree unless diagnosing a specific managed workspace.
- Read `.ddev-workspaces.toml` and confirm its declared `workspace_root` is ignored by Git.
- Check whether the repository is nested inside a different DDEV project. If so, use `[ddev.source_site]`; the CLI discovers the nearest containing DDEV site and manages generated sites under `~/.ddev-workspaces/sites`.
- Treat configuration `version = 1` as the configuration format, not the installed CLI version.
- Use `ddev-workspaces <command> --help` for the installed version's exact flags.

Read-only inspection is safe while investigating:

```sh
ddev-workspaces doctor
ddev-workspaces list
```

Address safety, ownership, path, and runtime problems reported by `doctor` before creating or removing a workspace. An intentionally dirty manager configuration may be accepted only for a user-authorized temporary or dogfood workflow as described below; do not bypass any other diagnostic.

## Configure a repository

When configuration is absent and the user wants the repository managed:

1. Add the intended workspace directory, normally `.worktrees/`, to `.gitignore`.
2. Create `.ddev-workspaces.toml` with a stable, DNS-safe `project_id` and repository-relative `workspace_root`.
3. Add `[ddev]`, prepared files, commands, and readiness checks only when supported by the repository's actual setup. Never invent secret values or expose sensitive command output.
4. Prefer committing the configuration and ignore rule on the appropriate branch so every agent sees the same durable setup. Never create that commit without user authorization. For an explicitly temporary or dogfood workflow, creation may proceed with intentional uncommitted configuration after warning the user that the configuration must remain unchanged until the workspace is removed.
5. Run `ddev-workspaces doctor` to validate the result.

Minimal configuration:

```toml
version = 1
project_id = "example-app"
workspace_root = ".worktrees"

[ddev]
app_root = "."
```

For a repository nested inside another DDEV site:

```toml
version = 1
project_id = "example-plugin"
workspace_root = ".worktrees"

[ddev]
app_root = "."

[ddev.source_site]
repository_path = "wp-content/plugins/example-plugin"
clone_database = true
```

Confirm the repository is beneath a regular, non-symlink `.ddev/config.yaml` and that `repository_path` identifies it from that containing site. The CLI creates the managed generated root when necessary, rejects absolute or escaping source-tree symlinks, clones the site, mounts the worktree at `repository_path`, disables Mutagen for the generated project, and restores a source project that was initially stopped.

Consult the [project README](https://github.com/alessandrotesoro/ddev-workspaces#configuration) for the full configuration schema.

## Create a workspace

Preview creation before applying it:

```sh
ddev-workspaces doctor
ddev-workspaces create --dry-run <name>
ddev-workspaces create <name>
ddev-workspaces list
```

Record the generated DDEV app root and deterministic project name printed by `create` or `list`. In source-site mode, the Git worktree is mounted inside that separate application but is not itself the DDEV app root. Run `ddev`, `ddev wp`, and other DDEV-backed commands from the generated app root reported by the CLI (or use an explicit DDEV project selector when the command supports one). Never run a bare DDEV command from the nested plugin worktree or source site: it may select the original source project instead.

Creation without `--base` uses the exact commit advertised by `origin`'s symbolic `HEAD`; it does not fetch. If that commit is missing, fetch the branch named by the error and repeat the dry run. Use `--base <revision>` only when the user or task requires a different locally resolvable base.

Use `--source-only` only when the task needs a Git worktree without file preparation, configured commands, readiness checks, or DDEV:

```sh
ddev-workspaces create --source-only <name>
```

For a plugin repository nested inside an existing WordPress/DDEV site, prefer source-site mode when the task needs a working WordPress runtime. Use source-only creation only when the task intentionally does not need the plugin mounted or a DDEV site.

## Diagnose a workspace

Use the path reported by the CLI:

```sh
ddev-workspaces doctor <path>
```

A failed creation may intentionally leave an ownership record, branch, and worktree for diagnosis. Fix the reported source, preparation command, readiness check, or DDEV problem, then run `doctor` again. Do not delete partial state manually.

## Remove a workspace

Removal is destructive and requires clear user intent. Inspect the owned target and preview removal before confirming the exact name:

```sh
ddev-workspaces list
ddev-workspaces remove --dry-run <name>
ddev-workspaces remove --confirm <name> <name>
```

Normal removal retains the Git branch and DDEV data. Delete DDEV data only when the user explicitly requests it, using both confirmations:

For source-site workspaces, normal removal also recursively deletes the exact generated DDEV application directory printed by the dry run. It does not delete the source site.

```sh
ddev-workspaces remove --delete-ddev-data \
  --confirm <name> --confirm-data <name> <name>
```

Never force removal, manually erase the ownership record, or prune Git worktree metadata to get around a refusal. Report the blocking diagnostic and ask for direction when it cannot be resolved safely.
