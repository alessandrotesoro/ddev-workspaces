mod support;

use std::fs;
use std::io::Write;

use support::{commit, init_repo, init_repo_with_origin, run_cli, stdout};

#[test]
fn omitted_base_uses_the_advertised_remote_head() {
    let (repository, _remote) = init_repo_with_origin();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    commit(repository.path(), "add omitted-base fixture");

    let output = run_cli(repository.path(), &["create", "--dry-run", "omitted"]);
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert!(output.status.success(), "{text}");
    assert!(text.contains("Base: refs/heads/main @"));
    assert!(!repository.path().join(".worktrees/omitted").exists());
}

#[test]
fn dry_run_is_read_only_for_a_valid_local_base() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    commit(repository.path(), "add dry-run fixture");

    let output = run_cli(
        repository.path(),
        &["create", "--dry-run", "--base", "HEAD", "dry-run"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert!(output.status.success(), "{text}");
    assert!(text.contains("no reservation"));
    assert!(!repository.path().join(".worktrees/dry-run").exists());
    assert!(
        !repository
            .path()
            .join(".git/ddev-workspaces/workspaces/dry-run.toml")
            .exists()
    );
}

#[test]
fn create_interruption_after_reservation_preserves_record_worktree_and_branch() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[files]]\nlabel = 'missing template'\ndestination = '.env'\ntemplate = 'missing.env.example'\n",
    );
    commit(repository.path(), "add failing preparation fixture");

    let output = run_cli(
        repository.path(),
        &["create", "--base", "HEAD", "interrupted"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));
    let workspace = repository.path().join(".worktrees/interrupted");
    let record = repository
        .path()
        .join(".git/ddev-workspaces/workspaces/interrupted.toml");

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("NOT READY"));
    assert!(text.contains("missing.env.example"));
    assert!(workspace.exists());
    assert!(record.exists());
    assert!(
        support::run_git(
            repository.path(),
            &["show-ref", "--verify", "--quiet", "refs/heads/interrupted"]
        )
        .status
        .success()
    );
}

#[test]
fn source_only_remove_requires_confirmation_and_retains_branch() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    commit(repository.path(), "add removal fixture");
    let created = run_cli(
        repository.path(),
        &["create", "--source-only", "--base", "HEAD", "remove-me"],
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    let rejected = run_cli(repository.path(), &["remove", "remove-me"]);
    let rejected_text = format!("{}{}", stdout(&rejected), support::stderr(&rejected));
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected_text.contains("requires `--confirm remove-me`"));
    assert!(repository.path().join(".worktrees/remove-me").exists());

    let dry_run = run_cli(repository.path(), &["remove", "--dry-run", "remove-me"]);
    let dry_text = stdout(&dry_run);
    assert!(dry_run.status.success(), "{dry_text}");
    assert!(dry_text.contains("DRY RUN"));
    assert!(repository.path().join(".worktrees/remove-me").exists());

    let removed = run_cli(
        repository.path(),
        &["remove", "--confirm", "remove-me", "remove-me"],
    );
    assert!(removed.status.success(), "{}", support::stderr(&removed));
    assert!(!repository.path().join(".worktrees/remove-me").exists());
    assert!(
        !repository
            .path()
            .join(".git/ddev-workspaces/workspaces/remove-me.toml")
            .exists()
    );
    assert!(
        support::run_git(
            repository.path(),
            &["show-ref", "--verify", "--quiet", "refs/heads/remove-me"]
        )
        .status
        .success()
    );
}

#[test]
fn list_ddev_identity_recomputes_source_only_status_without_scanning_other_repositories() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    );
    support::write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add list fixture");
    let created = run_cli(
        repository.path(),
        &["create", "--source-only", "--base", "HEAD", "listed"],
    );
    assert!(created.status.success(), "{}", support::stderr(&created));
    let workspace = repository.path().join(".worktrees/listed");
    fs::write(
        workspace.join("README.md"),
        "committed after workspace creation\n",
    )
    .expect("workspace change");
    assert!(
        support::run_git(&workspace, &["add", "README.md"])
            .status
            .success()
    );
    assert!(
        support::run_git(&workspace, &["commit", "-m", "advance workspace head"])
            .status
            .success()
    );

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let fake_bin = support::fake_ddev_directory(&fake_state.path().join("running"));
    let list = support::run_cli_with_path_and_vars(
        repository.path(),
        &["list"],
        fake_bin.path(),
        &[("DDEV_FAKE_NAME", "dw-fixture--listed")],
    );
    let text = format!("{}{}", stdout(&list), support::stderr(&list));

    assert!(list.status.success(), "{text}");
    assert!(text.contains("listed: SOURCE-ONLY"));

    fs::write(
        repository.path().join(".worktrees/listed/README.md"),
        "changed after creation\n",
    )
    .expect("make listed workspace not ready");
    let not_ready = support::run_cli_with_path_and_vars(
        repository.path(),
        &["list"],
        fake_bin.path(),
        &[("DDEV_FAKE_NAME", "dw-fixture--listed")],
    );
    let not_ready_text = format!("{}{}", stdout(&not_ready), support::stderr(&not_ready));
    assert_eq!(not_ready.status.code(), Some(1));
    assert!(not_ready_text.contains("NOT READY"));
}

#[test]
fn doctor_on_a_linked_worktree_uses_the_manager_configuration() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    let workspace = repository.path().join(".worktrees/linked");
    let created = run_cli(
        repository.path(),
        &["create", "--source-only", "--base", "HEAD", "linked"],
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    let workspace_value = workspace.to_str().expect("workspace path");
    let doctor = run_cli(repository.path(), &["doctor", workspace_value]);
    let text = format!("{}{}", stdout(&doctor), support::stderr(&doctor));

    assert!(doctor.status.success(), "{text}");
    assert!(text.contains("Configuration: READY"));
    assert!(text.ends_with("READY\n"));
}

#[test]
fn doctor_revalidates_semantic_ownership_record() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    commit(repository.path(), "add ownership validation fixture");
    let created = run_cli(
        repository.path(),
        &["create", "--source-only", "--base", "HEAD", "semantic"],
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    let record_path = repository
        .path()
        .join(".git/ddev-workspaces/workspaces/semantic.toml");
    let record = fs::read_to_string(&record_path).expect("ownership record");
    let invalid_record = record
        .lines()
        .map(|line| {
            if line.starts_with("base_sha = ") {
                "base_sha = 'invalid'".to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&record_path, format!("{invalid_record}\n")).expect("corrupted record");

    let workspace = repository.path().join(".worktrees/semantic");
    let workspace_value = workspace.to_str().expect("workspace path");
    let doctor = run_cli(repository.path(), &["doctor", workspace_value]);
    let text = format!("{}{}", stdout(&doctor), support::stderr(&doctor));

    assert_eq!(doctor.status.code(), Some(1));
    assert!(text.contains("Ownership: NOT READY"), "{text}");
    assert!(text.contains("base SHA"), "{text}");
}

#[test]
fn malformed_ownership_record_is_never_a_removal_authority() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    commit(repository.path(), "add malformed record fixture");
    let record_directory = repository.path().join(".git/ddev-workspaces/workspaces");
    fs::create_dir_all(&record_directory).expect("record directory");
    fs::write(
        record_directory.join("unmanaged.toml"),
        "version = 1\nproject_id = 'fixture'\nfuture = true\n",
    )
    .expect("malformed record");

    let doctor = run_cli(repository.path(), &["doctor"]);
    let doctor_text = format!("{}{}", stdout(&doctor), support::stderr(&doctor));
    assert_eq!(doctor.status.code(), Some(1));
    assert!(doctor_text.contains("Ownership: NOT READY"));

    let output = run_cli(
        repository.path(),
        &["remove", "--confirm", "unmanaged", "unmanaged"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("invalid") || text.contains("ownership record"));
}

#[test]
fn dirty_worktree_is_refused_before_removal() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    commit(repository.path(), "add dirty removal fixture");
    let created = run_cli(
        repository.path(),
        &["create", "--source-only", "--base", "HEAD", "dirty"],
    );
    assert!(created.status.success(), "{}", support::stderr(&created));
    fs::write(
        repository.path().join(".worktrees/dirty/untracked.txt"),
        "user work\n",
    )
    .expect("untracked worktree file");

    let output = run_cli(
        repository.path(),
        &["remove", "--confirm", "dirty", "dirty"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1));
    assert!(text.contains("dirty or has untracked files"));
    assert!(repository.path().join(".worktrees/dirty").exists());
    assert!(
        repository
            .path()
            .join(".git/ddev-workspaces/workspaces/dirty.toml")
            .exists()
    );
}

#[test]
fn main_worktree_alias_is_refused_even_with_a_record() {
    let repository = init_repo();
    let mut ignore = fs::OpenOptions::new()
        .append(true)
        .open(repository.path().join(".gitignore"))
        .expect("ignore file");
    ignore
        .write_all(b".ddev-workspaces-ignore-probe\n")
        .expect("ignore probe rule");
    commit(repository.path(), "add ignore probe rule");
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.'\n",
    );
    commit(repository.path(), "add main protection fixture");
    let head = String::from_utf8_lossy(
        &support::run_git(repository.path(), &["rev-parse", "HEAD"]).stdout,
    )
    .trim()
    .to_owned();
    let record_directory = repository.path().join(".git/ddev-workspaces/workspaces");
    fs::create_dir_all(&record_directory).expect("record directory");
    fs::write(
        record_directory.join("main.toml"),
        format!(
            "version = 1\nproject_id = 'fixture'\ncommon_directory = '{}'\nworktree_path = '{}'\nbase_sha = '{}'\nbranch = 'main'\nddev_name = 'dw-fixture--main'\n",
            fs::canonicalize(repository.path().join(".git"))
                .expect("canonical common directory")
                .display(),
            repository.path().display(),
            head
        ),
    )
    .expect("main ownership record");

    let output = run_cli(repository.path(), &["remove", "--confirm", "main", "main"]);
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text.contains("does not match configured workspace path"),
        "{text}"
    );
    assert!(
        repository
            .path()
            .join(".git/ddev-workspaces/workspaces/main.toml")
            .exists()
    );
}
