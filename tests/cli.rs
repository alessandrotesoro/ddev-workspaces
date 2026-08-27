mod support;

use support::{run_cli, stderr, stdout};

#[test]
fn top_level_help_lists_only_the_four_v1_commands() {
    let output = run_cli(std::path::Path::new("."), &["--help"]);
    let text = stdout(&output);

    assert!(output.status.success());
    assert!(text.contains("doctor"));
    assert!(text.contains("create"));
    assert!(text.contains("list"));
    assert!(text.contains("remove"));
    assert!(!text.contains("push"));
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

#[test]
fn command_help_exposes_dry_run_and_source_only() {
    let output = run_cli(std::path::Path::new("."), &["create", "--help"]);
    let text = stdout(&output);

    assert!(output.status.success());
    assert!(text.contains("--dry-run"));
    assert!(text.contains("--source-only"));
    assert!(text.contains("--base"));
}
