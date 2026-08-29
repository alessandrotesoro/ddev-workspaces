mod support;

use std::fs;

use support::{init_repo, run_cli, stdout};

#[test]
fn doctor_reports_unknown_configuration_fields_without_mutation() {
    let repository = init_repo();
    fs::write(
        repository.path().join(".ddev-workspaces.toml"),
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\nfuture = true\n",
    )
    .expect("configuration file");

    let output = run_cli(repository.path(), &["doctor"]);
    let text = stdout(&output);

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("Configuration: NOT READY"));
    assert!(text.contains("unknown field"));
    assert!(!repository.path().join(".worktrees").exists());
}

#[test]
fn doctor_accepts_a_strict_minimal_configuration_when_workspace_root_is_ignored() {
    let repository = init_repo();
    fs::write(
        repository.path().join(".ddev-workspaces.toml"),
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    )
    .expect("configuration file");

    let output = run_cli(repository.path(), &["doctor"]);
    let text = stdout(&output);

    assert!(output.status.success(), "{text}");
    assert!(text.contains("Configuration: READY"));
    assert!(text.ends_with("READY\n"));
}
