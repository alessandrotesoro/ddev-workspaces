mod support;

use std::fs;

use support::{init_repo, run_cli, stderr, stdout, write_laravel_config};

#[test]
fn doctor_rejects_unknown_fields_without_creating_state() {
    let repository = init_repo();
    fs::write(
        repository.path().join(".ddev-workspaces.toml"),
        "version = 2\nproject_id = 'fixture'\nworktree_root = '.ddev-workspaces/worktrees'\nfuture = true\n[runtime]\nadapter = 'laravel'\nhost = '.'\n",
    )
    .expect("configuration file");

    let output = run_cli(repository.path(), &["doctor"]);
    let text = stderr(&output);

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("invalid configuration"));
    assert!(!repository.path().join(".ddev-workspaces").exists());
}

#[test]
fn v2_configuration_is_strict_and_requires_a_running_host_for_readiness() {
    let repository = init_repo();
    write_laravel_config(repository.path(), "fresh");

    let output = run_cli(repository.path(), &["doctor"]);
    let text = stdout(&output);

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("Configuration: READY"), "{text}");
    assert!(text.contains("DDEV host: NOT READY"), "{text}");
}
