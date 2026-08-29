mod support;

use std::fs;

use support::{init_repo, init_repo_with_origin, run_cli, run_git, stdout, write_tracked_file};

#[test]
fn doctor_reports_hidden_index_flags_and_missing_tracked_paths() {
    let repository = init_repo();
    assert!(
        run_git(
            repository.path(),
            &["update-index", "--skip-worktree", "README.md"]
        )
        .status
        .success()
    );
    let output = run_cli(repository.path(), &["doctor"]);
    let text = stdout(&output);

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("hidden index flag"));
    assert!(text.contains("README.md"));

    assert!(
        run_git(
            repository.path(),
            &["update-index", "--no-skip-worktree", "README.md"],
        )
        .status
        .success()
    );
    fs::remove_file(repository.path().join("README.md")).expect("tracked path removal");
    let output = run_cli(repository.path(), &["doctor"]);
    assert!(stdout(&output).contains("tracked path missing"));
}

#[test]
fn create_without_base_uses_the_advertised_origin_head_sha() {
    let (repository, _remote) = init_repo_with_origin();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    support::commit(repository.path(), "add workspace config");
    let output = run_cli(repository.path(), &["create", "--dry-run", "task-1"]);
    let text = stdout(&output);

    assert!(
        output.status.success(),
        "stdout={text}\nstderr={}",
        support::stderr(&output)
    );
    assert!(text.contains("Base: refs/heads/main @"));
    assert!(!repository.path().join(".worktrees").exists());
}

#[test]
fn missing_origin_is_a_read_only_base_failure() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    support::commit(repository.path(), "add workspace config");
    let output = run_cli(repository.path(), &["create", "--dry-run", "task-1"]);
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("advertised HEAD"));
    assert!(text.contains("git fetch"));
    assert!(!repository.path().join(".worktrees").exists());
}

#[test]
fn explicit_local_base_does_not_require_an_origin() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    support::commit(repository.path(), "add workspace config");

    let output = run_cli(
        repository.path(),
        &["create", "--dry-run", "--base", "HEAD", "local-base"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert!(output.status.success(), "{text}");
    assert!(text.contains("Base: HEAD @"));
    assert!(!repository.path().join(".worktrees/local-base").exists());
}

#[test]
fn doctor_reports_sparse_checkout_assume_unchanged_and_worktree_diffs() {
    let repository = init_repo();
    assert!(
        run_git(
            repository.path(),
            &["update-index", "--assume-unchanged", "README.md"]
        )
        .status
        .success()
    );
    let assumed = run_cli(repository.path(), &["doctor"]);
    let assumed_text = stdout(&assumed);
    assert_eq!(assumed.status.code(), Some(1));
    assert!(assumed_text.contains("hidden index flag"));

    assert!(
        run_git(
            repository.path(),
            &["update-index", "--no-assume-unchanged", "README.md"]
        )
        .status
        .success()
    );
    fs::write(repository.path().join("README.md"), "changed fixture\n")
        .expect("changed tracked file");
    let changed = run_cli(repository.path(), &["doctor"]);
    let changed_text = stdout(&changed);
    assert_eq!(changed.status.code(), Some(1));
    assert!(
        changed_text.contains("working-tree files differ")
            || changed_text.contains("tracked content")
    );

    fs::write(repository.path().join("README.md"), "fixture\n").expect("restore tracked file");
    assert!(
        run_git(
            repository.path(),
            &["config", "core.sparseCheckout", "true"]
        )
        .status
        .success()
    );
    let sparse = run_cli(repository.path(), &["doctor"]);
    assert!(stdout(&sparse).contains("sparse checkout is enabled"));
}

#[test]
fn existing_branch_stops_dry_run_before_workspace_creation() {
    let repository = init_repo();
    write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    support::commit(repository.path(), "add workspace config");
    assert!(
        run_git(repository.path(), &["branch", "task-1"])
            .status
            .success()
    );

    let output = run_cli(
        repository.path(),
        &["create", "--dry-run", "--base", "HEAD", "task-1"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("already exists"));
    assert!(!repository.path().join(".worktrees/task-1").exists());
}
