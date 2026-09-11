mod support;

use support::{run_cli, stderr, stdout};

#[test]
fn top_level_help_lists_shared_host_commands() {
    let output = run_cli(std::path::Path::new("."), &["--help"]);
    let text = stdout(&output);

    assert!(output.status.success());
    assert!(text.contains("doctor"));
    assert!(text.contains("host"));
    assert!(text.contains("create"));
    assert!(text.contains("list"));
    assert!(text.contains("remove"));
}

#[test]
fn create_help_exposes_base_fresh_and_dry_run() {
    let output = run_cli(std::path::Path::new("."), &["create", "--help"]);
    let text = stdout(&output);

    assert!(output.status.success());
    assert!(text.contains("--dry-run"));
    assert!(text.contains("--fresh"));
    assert!(text.contains("--base"));
}

#[test]
fn remove_help_requires_explicit_database_deletion_flag() {
    let output = run_cli(std::path::Path::new("."), &["remove", "--help"]);
    let text = stdout(&output);

    assert!(output.status.success());
    assert!(text.contains("--delete-database"));
    assert!(text.contains("--yes"));
}

#[test]
fn unknown_cli_arguments_use_usage_exit_code_two() {
    let output = run_cli(
        std::path::Path::new("."),
        &["create", "--unknown", "task-1"],
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("unexpected argument"));
}
