mod support;

use std::fs;

use support::{commit, init_repo, run_cli, run_cli_with_path_and_vars, stdout};

#[test]
fn source_only_skips_files_commands_and_ddev() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n\n[[files]]\nlabel = 'environment'\ndestination = '.env'\nsource_env = 'FIXTURE_SOURCE_ONLY_ENV'\n\n[[commands]]\nlabel = 'marker'\ncwd = '.'\nargv = ['touch', 'runtime-marker']\n",
    );
    write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add source-only fixture");

    let fake_state_directory = tempfile::tempdir().expect("fake DDEV state directory");
    let fake_state = fake_state_directory.path().join("running");
    let fake = support::fake_ddev_directory(&fake_state);
    let output = run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--source-only", "--base", "HEAD", "source-only"],
        fake.path(),
        &[("DDEV_FAKE_NAME", "dw-fixture--source-only")],
    );
    let text = stdout(&output);

    assert!(output.status.success(), "{text}");
    assert!(text.contains("READY — source-only workspace"));
    assert!(
        !repository
            .path()
            .join(".worktrees/source-only/.env")
            .exists()
    );
    assert!(
        !repository
            .path()
            .join(".worktrees/source-only/runtime-marker")
            .exists()
    );
    assert!(!fake_state.exists());
}

#[test]
fn named_files_and_commands_produce_runtime_readiness() {
    let repository = init_repo();
    fs::write(
        repository.path().join(".gitignore"),
        ".worktrees/\n.ddev/config.ddev-workspaces.yaml\n.env\nfirst-marker\nsecond-marker\n",
    )
    .expect("fixture ignore file");
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[files]]\nlabel = 'environment'\ndestination = '.env'\ntemplate = '.env.example'\n\n[[commands]]\nlabel = 'first marker'\ncwd = '.'\nargv = ['touch', 'first-marker']\n\n[[commands]]\nlabel = 'second marker'\ncwd = '.'\nargv = ['touch', 'second-marker']\n\n[[checks]]\nlabel = 'first marker exists'\nkind = 'path-exists'\npath = 'first-marker'\n\n[[checks]]\nlabel = 'environment key'\nkind = 'env-key'\npath = '.env'\nkey = 'APP_KEY'\n",
    );
    write_tracked_file(repository.path(), ".env.example", "APP_KEY=fixture-value\n");
    commit(repository.path(), "add preparation fixture");

    let output = run_cli(repository.path(), &["create", "--base", "HEAD", "prepared"]);
    let text = stdout(&output);
    let workspace = repository.path().join(".worktrees/prepared");

    assert!(output.status.success(), "{text}");
    assert!(text.contains("Runtime: READY"));
    assert_eq!(
        fs::read_to_string(workspace.join(".env")).expect("copied environment"),
        "APP_KEY=fixture-value\n"
    );
    assert!(workspace.join("first-marker").is_file());
    assert!(workspace.join("second-marker").is_file());
}

#[test]
fn non_ignored_file_destinations_are_rejected_before_publication() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[files]]\nlabel = 'runtime configuration'\ndestination = 'runtime.conf'\ntemplate = 'runtime.conf.example'\n",
    );
    write_tracked_file(repository.path(), "runtime.conf.example", "runtime=true\n");
    commit(repository.path(), "add non-ignored file fixture");

    let output = run_cli(
        repository.path(),
        &["create", "--base", "HEAD", "non-ignored-file"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));
    let workspace = repository.path().join(".worktrees/non-ignored-file");

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("is not ignored by Git"), "{text}");
    assert!(!text.contains("Runtime: READY"));
    assert!(!workspace.join("runtime.conf").exists());
    assert!(workspace.exists());
}

#[test]
fn non_ignored_command_outputs_prevent_create_and_list_from_reporting_ready() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[commands]]\nlabel = 'generate runtime configuration'\ncwd = '.'\nargv = ['touch', 'generated.conf']\n",
    );
    commit(repository.path(), "add non-ignored command fixture");

    let created = run_cli(
        repository.path(),
        &["create", "--base", "HEAD", "non-ignored-command"],
    );
    let created_text = format!("{}{}", stdout(&created), support::stderr(&created));

    assert_eq!(created.status.code(), Some(1));
    assert!(created_text.contains("dirty or has untracked files"));
    assert!(!created_text.contains("Runtime: READY"));

    let listed = run_cli(repository.path(), &["list"]);
    let listed_text = format!("{}{}", stdout(&listed), support::stderr(&listed));

    assert_eq!(listed.status.code(), Some(1));
    assert!(listed_text.contains("non-ignored-command: NOT READY"));
    assert!(listed_text.contains("dirty or has untracked files"));
}

#[test]
fn ignored_command_outputs_remain_ready_and_removable() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[commands]]\nlabel = 'generate ignored environment'\ncwd = '.'\nargv = ['touch', '.env']\n",
    );
    commit(repository.path(), "add ignored command fixture");

    let created = run_cli(
        repository.path(),
        &["create", "--base", "HEAD", "ignored-command"],
    );
    let created_text = format!("{}{}", stdout(&created), support::stderr(&created));
    assert!(created.status.success(), "{created_text}");

    let listed = run_cli(repository.path(), &["list"]);
    let listed_text = format!("{}{}", stdout(&listed), support::stderr(&listed));
    assert!(listed.status.success(), "{listed_text}");
    assert!(listed_text.contains("ignored-command: READY"));

    let removed = run_cli(repository.path(), &["remove", "--yes", "ignored-command"]);
    let removed_text = format!("{}{}", stdout(&removed), support::stderr(&removed));
    assert!(removed.status.success(), "{removed_text}");
}

#[test]
fn local_file_sources_are_child_only_and_private() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[files]]\nlabel = 'local environment'\ndestination = '.env'\nsource_env = 'FIXTURE_ENV_SOURCE'\n",
    );
    commit(repository.path(), "add local source fixture");
    let source = tempfile::tempdir().expect("source directory");
    let source_file = source.path().join("local.env");
    fs::write(&source_file, "APP_KEY=local-value\n").expect("source file");

    let source_value = source_file.to_str().expect("source path");
    let output = support::run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--base", "HEAD", "local-source"],
        repository.path(),
        &[("FIXTURE_ENV_SOURCE", source_value)],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));
    let destination = repository.path().join(".worktrees/local-source/.env");

    assert!(output.status.success(), "{text}");
    assert_eq!(
        fs::read_to_string(&destination).expect("copied local source"),
        "APP_KEY=local-value\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(destination)
                .expect("destination metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(!text.contains(source_value));
    assert!(!text.contains("local-value"));
}

#[test]
fn missing_local_source_fails_before_reservation() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[files]]\nlabel = 'missing local environment'\ndestination = '.env'\nsource_env = 'FIXTURE_MISSING_SOURCE'\n",
    );
    commit(repository.path(), "add missing source fixture");

    let output = run_cli(
        repository.path(),
        &["create", "--base", "HEAD", "missing-source"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("FIXTURE_MISSING_SOURCE"));
    assert!(!repository.path().join(".worktrees/missing-source").exists());
    assert!(
        !repository
            .path()
            .join(".git/ddev-workspaces/workspaces/missing-source.toml")
            .exists()
    );
}

#[test]
fn runtime_readiness_is_rechecked_after_declared_commands() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[files]]\nlabel = 'environment'\ndestination = '.env'\ntemplate = '.env.example'\n\n[[commands]]\nlabel = 'remove environment'\ncwd = '.'\nargv = ['rm', '.env']\n",
    );
    write_tracked_file(repository.path(), ".env.example", "APP_KEY=fixture-value\n");
    commit(repository.path(), "add post-command readiness fixture");

    let output = run_cli(repository.path(), &["create", "--base", "HEAD", "removed"]);
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("runtime readiness failed"), "{text}");
    assert!(!text.contains("Runtime: READY"));
    assert!(repository.path().join(".worktrees/removed").exists());
    assert!(
        repository
            .path()
            .join(".git/ddev-workspaces/workspaces/removed.toml")
            .exists()
    );
}

#[test]
fn git_readiness_is_rechecked_after_declared_commands() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[commands]]\nlabel = 'remove tracked source'\ncwd = '.'\nargv = ['rm', 'README.md']\n",
    );
    commit(repository.path(), "add post-command Git fixture");

    let output = run_cli(
        repository.path(),
        &["create", "--base", "HEAD", "git-check"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("source verification failed"), "{text}");
    assert!(text.contains("tracked path missing"), "{text}");
    assert!(!text.contains("Runtime: READY"));
}

fn write_tracked_file(root: &std::path::Path, relative: &str, contents: &str) {
    support::write_tracked_file(root, relative, contents);
}
