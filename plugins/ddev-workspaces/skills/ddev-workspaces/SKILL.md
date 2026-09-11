---
name: ddev-workspaces
description: Create, inspect, diagnose, and safely remove local Git worktrees managed by the ddev-workspaces CLI on a shared DDEV host. Use when a task calls for an isolated DDEV workspace or refers to .ddev-workspaces.toml, managed workspaces, or ddev-workspaces commands.
---

# ddev-workspaces

Use `ddev-workspaces` for isolated Git worktrees that share one existing DDEV application. Let the CLI enforce ownership and safety; do not replace its lifecycle with raw `git worktree`, filesystem deletion, or manual database cleanup.

## Before operating

- Confirm `ddev-workspaces` is installed. If it is missing, direct the user to the project installation instructions.
- Work from the repository's main worktree unless diagnosing a specific managed workspace.
- Read `.ddev-workspaces.toml` and confirm its declared `worktree_root` is ignored by Git when it is inside the repository.
- Confirm the configured `runtime.host` contains the repository and has a regular `.ddev/config.yaml`.
- Treat `version = 2` as the configuration format. Unknown fields and older schemas are not compatible.
- Use `ddev-workspaces <command> --help` for the installed binary's exact flags.

Read-only inspection is safe while investigating:

```sh
ddev-workspaces doctor
ddev-workspaces list
```

Address safety, ownership, path, host identity, and routing problems reported by `doctor` before creating or removing a workspace. Do not bypass diagnostics or edit state records manually.

## Configure a repository

When configuration is absent and the user wants the repository managed:

1. Add `.ddev-workspaces/` or the configured worktree root to `.gitignore` when the path is inside the repository.
2. Create `.ddev-workspaces.toml` with a stable DNS-safe `project_id`, `worktree_root`, and strict runtime adapter.
3. Use `laravel` when the repository is the DDEV host root. Use `wordpress-component` when the repository is a plugin or theme nested in the host.
4. Add only repository-specific environment, carry, provisioning, check, and writable rules. Never invent secrets or expose sensitive command output.
5. Do not create a configuration commit without user authorization. For a temporary workflow, warn that the configuration and ignore rule must remain unchanged until the workspace is removed.
6. Initialize the existing running host, then run `ddev-workspaces doctor`. Host initialization is the only lifecycle command that may restart DDEV, and only when the managed host assets change.

Laravel example:

```toml
version = 2
project_id = "example-app"
worktree_root = ".ddev-workspaces/worktrees"

[runtime]
adapter = "laravel"
host = "."
docroot = "public"

[database]
default = "copy"
```

Nested WordPress plugin example:

```toml
version = 2
project_id = "example-plugin"
worktree_root = "../../../.ddev-workspaces/worktrees"

[runtime]
adapter = "wordpress-component"
host = "../../../"
component_kind = "plugin"
component_path = "wp-content/plugins/example-plugin"

[database]
default = "copy"
```

The component path must resolve exactly to the repository and must be below `wp-content/plugins/` or `wp-content/themes/` according to `component_kind`. The CLI creates a mount-safe runtime view with relative links, an isolated writable content area, and an owned workspace `wp-config.php`.

Initialize the shared host once. This explicit command may briefly restart DDEV when it installs or updates the managed assets; ordinary create, list, doctor, and remove operations do not restart the host.

```sh
ddev start
ddev-workspaces host init
ddev-workspaces doctor
```

## Create a workspace

Preview creation before applying it:

```sh
ddev-workspaces create --dry-run <name>
ddev-workspaces create <name>
ddev-workspaces list
```

Each workspace uses a hostname below the configured host, for example `https://<resource-key>.<host>.ddev.site`, while the DDEV web and database container identities remain those of the shared host. Do not start a second DDEV project for a workspace.

Creation without `--base` uses the exact commit advertised by the local `origin` symbolic `HEAD`; it does not fetch. Use `--base <revision>` only when the user or task requires a different locally resolvable base. Use `--fresh` when the workspace must start with an empty database.

## Diagnose a workspace

Use the path reported by the CLI:

```sh
ddev-workspaces doctor <path>
```

A failed creation may retain an error record, branch, worktree, database, route, or generated view when those resources were already owned. Fix the reported source, command, check, or host problem, then retry or remove through the CLI. Never delete partial state manually.

## Remove a workspace

Removal is destructive and requires clear user intent. Inspect the target and preview removal first:

```sh
ddev-workspaces list
ddev-workspaces remove --dry-run <name>
ddev-workspaces remove <name>
```

Copied databases are retained by default as `retained-data`; fresh databases are deleted automatically. Delete retained data only when the user explicitly requests it:

```sh
ddev-workspaces remove --delete-database <name>
```

In a non-interactive environment, add `--yes` only after the user has authorized the removal:

```sh
ddev-workspaces remove --yes <name>
ddev-workspaces remove --delete-database --yes <name>
```

Never force removal, erase ownership records, prune Git metadata, or stop the shared DDEV host to get around a refusal. Report the blocking diagnostic and ask for direction when it cannot be resolved safely.
