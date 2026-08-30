mod support;

use std::fs;
use std::io::Write;

use support::{commit, init_repo, run_cli, stdout};

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
fn dry_run_rejects_a_template_missing_from_the_selected_base_before_reservation() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[files]]\nlabel = 'missing template'\ndestination = '.env'\ntemplate = 'missing.env.example'\n",
    );
    commit(repository.path(), "add missing template fixture");

    let output = run_cli(
        repository.path(),
        &["create", "--dry-run", "--base", "HEAD", "missing-template"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("regular tracked Git file in base commit"),
        "{text}"
    );
    assert!(
        !repository
            .path()
            .join(".worktrees/missing-template")
            .exists()
    );
    assert!(
        !repository
            .path()
            .join(".git/ddev-workspaces/workspaces/missing-template.toml")
            .exists()
    );
    assert!(
        !support::run_git(
            repository.path(),
            [
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/missing-template"
            ]
            .as_slice(),
        )
        .status
        .success()
    );
}

#[test]
fn dry_run_accepts_a_regular_tracked_template_in_the_selected_base() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[files]]\nlabel = 'environment'\ndestination = '.env'\ntemplate = '.env.example'\n",
    );
    support::write_tracked_file(repository.path(), ".env.example", "APP_KEY=fixture\n");
    commit(repository.path(), "add tracked template fixture");

    let output = run_cli(
        repository.path(),
        &["create", "--dry-run", "--base", "HEAD", "tracked-template"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert!(output.status.success(), "{text}");
    assert!(text.contains("READY — dry run complete"), "{text}");
    assert!(
        !repository
            .path()
            .join(".worktrees/tracked-template")
            .exists()
    );
    assert!(
        !repository
            .path()
            .join(".git/ddev-workspaces/workspaces/tracked-template.toml")
            .exists()
    );
}

#[test]
fn create_failure_after_reservation_preserves_record_worktree_and_branch() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[files]]\nlabel = 'non-ignored runtime file'\ndestination = 'runtime.env'\ntemplate = 'runtime.env.example'\n",
    );
    support::write_tracked_file(repository.path(), "runtime.env.example", "READY=true\n");
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
    assert!(text.contains("is not ignored by Git"));
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
fn source_only_removal_dry_run_rejects_data_deletion_without_owned_ddev() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    );
    commit(repository.path(), "add data deletion preflight fixture");
    let created = run_cli(
        repository.path(),
        &[
            "create",
            "--source-only",
            "--base",
            "HEAD",
            "source-only-data",
        ],
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    let output = run_cli(
        repository.path(),
        &[
            "remove",
            "--dry-run",
            "--delete-ddev-data",
            "source-only-data",
        ],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("exact owned DDEV identity is absent"),
        "{text}"
    );
    assert!(
        !text.contains("READY — removal preflight complete"),
        "{text}"
    );
    assert!(
        repository
            .path()
            .join(".worktrees/source-only-data")
            .exists()
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
fn list_keeps_a_failed_full_creation_not_ready() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[[checks]]\nlabel = 'required artifact'\nkind = 'path-exists'\npath = 'missing-artifact'\n",
    );
    commit(repository.path(), "add failing runtime check");

    let created = run_cli(
        repository.path(),
        &["create", "--base", "HEAD", "failed-full"],
    );
    assert_eq!(created.status.code(), Some(1));
    assert!(repository.path().join(".worktrees/failed-full").exists());

    let listed = run_cli(repository.path(), &["list"]);
    let text = format!("{}{}", stdout(&listed), support::stderr(&listed));

    assert_eq!(listed.status.code(), Some(1), "{text}");
    assert!(text.contains("failed-full: NOT READY"), "{text}");
    assert!(!text.contains("failed-full: SOURCE-ONLY"), "{text}");
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
fn doctor_skips_runtime_and_ddev_for_a_verified_source_only_workspace() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n\n[[files]]\nlabel = 'environment'\ndestination = '.env'\ntemplate = '.env.example'\n\n[[checks]]\nlabel = 'environment key'\nkind = 'env-key'\npath = '.env'\nkey = 'APP_KEY'\n",
    );
    support::write_tracked_file(repository.path(), ".env.example", "APP_KEY=fixture\n");
    support::write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add source-only doctor fixture");

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let fake_log = fake_state.path().join("calls.log");
    let fake_bin = support::fake_ddev_directory(&fake_state.path().join("running"));
    let variables = [
        ("DDEV_FAKE_NAME", "dw-fixture--doctor-source-only"),
        ("DDEV_FAKE_LOG", fake_log.to_str().expect("fake log path")),
    ];
    let created = support::run_cli_with_path_and_vars(
        repository.path(),
        &[
            "create",
            "--source-only",
            "--base",
            "HEAD",
            "doctor-source-only",
        ],
        fake_bin.path(),
        &variables,
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    let workspace = repository.path().join(".worktrees/doctor-source-only");
    let workspace_value = workspace.to_str().expect("workspace path");
    let doctor = support::run_cli_with_path_and_vars(
        repository.path(),
        &["doctor", workspace_value],
        fake_bin.path(),
        &variables,
    );
    let text = format!("{}{}", stdout(&doctor), support::stderr(&doctor));

    assert!(doctor.status.success(), "{text}");
    assert!(text.contains("Runtime: skipped by --source-only"), "{text}");
    assert!(text.contains("DDEV: skipped by --source-only"), "{text}");
    assert!(!workspace.join(".env").exists());
    assert!(!fake_log.exists());
    assert!(text.ends_with("READY\n"), "{text}");
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
            "version = 1\nproject_id = 'fixture'\ncommon_directory = '{}'\nworktree_path = '{}'\nbase_sha = '{}'\nbranch = 'main'\nddev_name = 'dw-fixture--main'\nsource_only = true\n",
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
