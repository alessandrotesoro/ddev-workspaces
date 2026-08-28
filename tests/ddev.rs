mod support;

use std::fs;

use support::{commit, init_repo, run_cli_with_path_and_vars, run_git, stdout};

#[test]
fn full_creation_and_default_removal_use_the_exact_fake_identity() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    );
    support::write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add DDEV fixture");

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let state = fake_state.path().join("running");
    let fake_log = fake_state.path().join("calls.log");
    let fake_bin = support::fake_ddev_directory(&state);
    let variables = [
        ("DDEV_FAKE_NAME", "dw-fixture--task-1"),
        ("DDEV_FAKE_LOG", fake_log.to_str().expect("fake log path")),
    ];

    let created = run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--base", "HEAD", "task-1"],
        fake_bin.path(),
        &variables,
    );
    let created_text = format!("{}{}", stdout(&created), support::stderr(&created));
    let workspace = repository.path().join(".worktrees/task-1");

    assert!(created.status.success(), "{created_text}");
    assert!(created_text.contains("DDEV: dw-fixture--task-1"));
    assert_eq!(
        fs::read_to_string(workspace.join(".ddev/config.ddev-workspaces.yaml"))
            .expect("owned DDEV override"),
        "name: dw-fixture--task-1\n"
    );
    assert!(state.exists());

    let duplicate = run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--base", "HEAD", "task-1"],
        fake_bin.path(),
        &variables,
    );
    let duplicate_text = format!("{}{}", stdout(&duplicate), support::stderr(&duplicate));
    assert_eq!(duplicate.status.code(), Some(1));
    assert!(
        duplicate_text.contains("already exists") || duplicate_text.contains("already registered")
    );

    let removed = run_cli_with_path_and_vars(
        repository.path(),
        &["remove", "--confirm", "task-1", "task-1"],
        fake_bin.path(),
        &variables,
    );
    let removed_text = format!("{}{}", stdout(&removed), support::stderr(&removed));
    assert!(removed.status.success(), "{removed_text}");
    assert!(!workspace.exists());
    assert!(!state.exists());
    assert!(
        run_git(
            repository.path(),
            &["show-ref", "--verify", "--quiet", "refs/heads/task-1"]
        )
        .status
        .success()
    );

    let calls = fs::read_to_string(fake_log).expect("fake DDEV call log");
    assert_eq!(
        calls
            .lines()
            .filter(|line| *line == "list --json-output")
            .count(),
        7
    );
    assert!(
        calls
            .lines()
            .any(|line| line == "stop --unlist dw-fixture--task-1")
    );
    assert!(!calls.lines().any(|line| line.contains("delete")));
}

#[test]
fn ddev_data_removal_requires_two_exact_confirmations() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    );
    support::write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add DDEV data fixture");

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let state = fake_state.path().join("running");
    let fake_log = fake_state.path().join("calls.log");
    let fake_bin = support::fake_ddev_directory(&state);
    let variables = [
        ("DDEV_FAKE_NAME", "dw-fixture--data-test"),
        ("DDEV_FAKE_LOG", fake_log.to_str().expect("fake log path")),
    ];
    let created = run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--base", "HEAD", "data-test"],
        fake_bin.path(),
        &variables,
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    let missing_data_confirmation = run_cli_with_path_and_vars(
        repository.path(),
        &[
            "remove",
            "--delete-ddev-data",
            "--confirm",
            "data-test",
            "data-test",
        ],
        fake_bin.path(),
        &variables,
    );
    let missing_text = format!(
        "{}{}",
        stdout(&missing_data_confirmation),
        support::stderr(&missing_data_confirmation)
    );
    assert_eq!(missing_data_confirmation.status.code(), Some(1));
    assert!(missing_text.contains("second exact confirmation"));
    assert!(state.exists());

    let removed = run_cli_with_path_and_vars(
        repository.path(),
        &[
            "remove",
            "--delete-ddev-data",
            "--confirm",
            "data-test",
            "--confirm-data",
            "data-test",
            "data-test",
        ],
        fake_bin.path(),
        &variables,
    );
    let removed_text = format!("{}{}", stdout(&removed), support::stderr(&removed));
    assert!(removed.status.success(), "{removed_text}");
    let calls = fs::read_to_string(fake_log).expect("fake DDEV call log");
    assert!(
        calls
            .lines()
            .any(|line| { line == "stop --remove-data --unlist dw-fixture--data-test" })
    );
}

#[test]
fn post_start_identity_failure_preserves_owned_state() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    );
    support::write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add failing DDEV fixture");

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let state = fake_state.path().join("running");
    let fake_bin = support::fake_ddev_directory(&state);
    let output = run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--base", "HEAD", "stopped"],
        fake_bin.path(),
        &[
            ("DDEV_FAKE_NAME", "dw-fixture--stopped"),
            ("DDEV_FAKE_STATUS", "stopped"),
        ],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("not running"));
    assert!(text.contains("Ownership record preserved"));
    assert!(repository.path().join(".worktrees/stopped").exists());
    assert!(
        repository
            .path()
            .join(".git/ddev-workspaces/workspaces/stopped.toml")
            .exists()
    );
}

#[test]
fn create_dry_run_with_ddev_never_starts_or_reserves() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    );
    support::write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add DDEV dry-run fixture");

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let state = fake_state.path().join("running");
    let fake_log = fake_state.path().join("calls.log");
    let fake_bin = support::fake_ddev_directory(&state);
    let output = run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--dry-run", "--base", "HEAD", "dry-run"],
        fake_bin.path(),
        &[
            ("DDEV_FAKE_NAME", "dw-fixture--dry-run"),
            ("DDEV_FAKE_LOG", fake_log.to_str().expect("fake log path")),
        ],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert!(output.status.success(), "{text}");
    assert!(text.contains("dry run"));
    assert!(!state.exists());
    assert!(!repository.path().join(".worktrees/dry-run").exists());
    assert!(
        !repository
            .path()
            .join(".git/ddev-workspaces/workspaces/dry-run.toml")
            .exists()
    );
    let calls = fs::read_to_string(fake_log).expect("fake DDEV call log");
    assert!(!calls.lines().any(|line| line == "start"));
}
