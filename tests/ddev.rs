mod support;

use std::fs;
use std::path::PathBuf;

use support::{commit, init_repo, run_cli_with_path_and_vars, run_git, stdout};
use tempfile::TempDir;

fn external_repository(source_env: &str, generated_env: &str) -> (TempDir, PathBuf) {
    let site = tempfile::tempdir().expect("external DDEV site");
    let repository = site.path().join("wp-content/plugins/fixture");
    fs::create_dir_all(&repository).expect("repository directory");
    assert!(
        run_git(&repository, &["init", "-b", "main"])
            .status
            .success()
    );
    assert!(
        run_git(&repository, &["config", "user.name", "Fixture User"])
            .status
            .success()
    );
    assert!(
        run_git(
            &repository,
            &["config", "user.email", "fixture@example.test"]
        )
        .status
        .success()
    );
    fs::write(repository.join(".gitignore"), ".worktrees/\n").expect("ignore rule");
    fs::write(repository.join("README.md"), "fixture\n").expect("fixture source");
    fs::write(
        repository.join(".ddev-workspaces.toml"),
        format!(
            "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n\n[ddev.external_site]\nsource_root_env = '{source_env}'\ngenerated_root_env = '{generated_env}'\nrepository_path = 'wp-content/plugins/fixture'\nclone_database = false\n"
        ),
    )
    .expect("workspace configuration");
    commit(&repository, "external site fixture");
    fs::create_dir_all(site.path().join(".ddev")).expect("DDEV directory");
    fs::write(site.path().join(".ddev/config.yaml"), "type: wordpress\n")
        .expect("DDEV configuration");
    (site, repository)
}

#[test]
fn external_site_creation_mounts_the_site_and_plugin_worktree_and_clones_database() {
    let site = tempfile::tempdir().expect("external DDEV site");
    let repository = site.path().join("wp-content/plugins/fixture");
    fs::create_dir_all(repository.join(".ddev-placeholder")).expect("repository directory");
    assert!(
        run_git(&repository, &["init", "-b", "main"])
            .status
            .success()
    );
    assert!(
        run_git(&repository, &["config", "user.name", "Fixture User"])
            .status
            .success()
    );
    assert!(
        run_git(
            &repository,
            &["config", "user.email", "fixture@example.test"]
        )
        .status
        .success()
    );
    fs::write(repository.join(".gitignore"), ".worktrees/\n.ddev-site/\n").expect("ignore rules");
    fs::write(repository.join("README.md"), "fixture\n").expect("fixture source");
    fs::write(
        repository.join(".ddev-workspaces.toml"),
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n\n[ddev.external_site]\nsource_root_env = 'FIXTURE_DDEV_SITE'\ngenerated_root_env = 'FIXTURE_DDEV_GENERATED_ROOT'\nrepository_path = 'wp-content/plugins/fixture'\nclone_database = true\n",
    )
    .expect("workspace configuration");
    commit(&repository, "external site fixture");
    fs::create_dir_all(site.path().join(".ddev")).expect("DDEV directory");
    fs::write(site.path().join(".ddev/config.yaml"), "type: wordpress\n")
        .expect("DDEV configuration");
    fs::write(site.path().join("index.php"), "<?php // source site\n").expect("source site file");
    fs::create_dir_all(site.path().join("wp-content/plugins/dependency/.git"))
        .expect("dependency metadata");
    fs::create_dir_all(
        site.path()
            .join("wp-content/plugins/dependency/node_modules"),
    )
    .expect("dependency node modules");
    fs::write(
        site.path().join("wp-content/plugins/dependency/plugin.php"),
        "<?php // dependency\n",
    )
    .expect("dependency plugin");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        "plugin.php",
        site.path()
            .join("wp-content/plugins/dependency/plugin-link.php"),
    )
    .expect("safe relative dependency symlink");

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let generated_sites = tempfile::tempdir().expect("generated DDEV sites");
    let generated_root = generated_sites.path().join("new/generated-root");
    let state = fake_state.path().join("running");
    let log = fake_state.path().join("calls.log");
    let fake_bin = support::fake_ddev_directory(&state);
    let variables = [
        ("DDEV_FAKE_NAME", "dw-fixture--nested"),
        ("DDEV_FAKE_LOG", log.to_str().expect("log path")),
        (
            "FIXTURE_DDEV_SITE",
            site.path().to_str().expect("site path"),
        ),
        (
            "FIXTURE_DDEV_GENERATED_ROOT",
            generated_root.to_str().expect("generated site root"),
        ),
    ];
    let output = run_cli_with_path_and_vars(
        &repository,
        &["create", "--base", "HEAD", "nested"],
        fake_bin.path(),
        &variables,
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));
    assert!(output.status.success(), "{text}");
    let workspace = repository.join(".worktrees/nested");
    let generated_site = generated_root.join("fixture/nested");
    let compose =
        fs::read_to_string(generated_site.join(".ddev/docker-compose.ddev-workspaces.yaml"))
            .expect("generated compose configuration");
    let canonical_generated_site =
        fs::canonicalize(&generated_site).expect("canonical generated site");

    assert_eq!(
        fs::read_to_string(generated_site.join("index.php")).expect("cloned site file"),
        "<?php // source site\n"
    );
    assert!(
        generated_site
            .join("wp-content/plugins/dependency/plugin.php")
            .is_file()
    );
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(generated_site.join("wp-content/plugins/dependency/plugin-link.php"))
            .expect("copied relative dependency symlink"),
        PathBuf::from("plugin.php")
    );
    assert!(
        !generated_site
            .join("wp-content/plugins/dependency/.git")
            .exists()
    );
    assert!(
        !generated_site
            .join("wp-content/plugins/dependency/node_modules")
            .exists()
    );
    assert!(
        !generated_site
            .join("wp-content/plugins/fixture/README.md")
            .exists()
    );
    assert_eq!(
        compose
            .lines()
            .filter(|line| line.trim_start().starts_with('-'))
            .count(),
        1
    );
    assert!(compose.contains(&format!(
        "{}:/var/www/html/wp-content/plugins/fixture",
        workspace.display()
    )));
    let calls = fs::read_to_string(&log).expect("DDEV calls");
    assert!(calls.lines().any(|line| line.starts_with("export-db ")));
    assert!(calls.lines().any(|line| line.starts_with("import-db ")));

    assert!(text.contains(&canonical_generated_site.display().to_string()));

    let removal_preview = run_cli_with_path_and_vars(
        &repository,
        &["remove", "--dry-run", "nested"],
        fake_bin.path(),
        &variables,
    );
    assert!(
        removal_preview.status.success(),
        "{}",
        support::stderr(&removal_preview)
    );
    let preview_text = stdout(&removal_preview);
    assert!(
        preview_text.contains(&format!(
            "Will recursively remove generated DDEV application {}.",
            canonical_generated_site.display()
        )),
        "{preview_text}"
    );

    let removed = run_cli_with_path_and_vars(
        &repository,
        &["remove", "--confirm", "nested", "nested"],
        fake_bin.path(),
        &variables,
    );
    let removed_text = format!("{}{}", stdout(&removed), support::stderr(&removed));
    assert!(removed.status.success(), "{removed_text}");
    assert!(!workspace.exists());
    assert!(!generated_site.exists());
}

#[test]
fn external_source_only_creation_needs_no_ddev_environment() {
    let (_site, repository) =
        external_repository("SOURCE_ONLY_DDEV_SITE", "SOURCE_ONLY_GENERATED_ROOT");

    let output = support::run_cli(
        &repository,
        &["create", "--source-only", "--base", "HEAD", "source-only"],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert!(output.status.success(), "{text}");
    assert!(repository.join(".worktrees/source-only").is_dir());
    assert!(text.contains("DDEV: skipped by --source-only"));
}

#[cfg(unix)]
#[test]
fn external_creation_rejects_a_symlinked_source_ddev_directory_without_mutating_it() {
    use std::os::unix::fs::symlink;

    let (site, repository) = external_repository("SYMLINK_DDEV_SITE", "SYMLINK_GENERATED_ROOT");
    let external_ddev = tempfile::tempdir().expect("external DDEV target");
    fs::remove_dir_all(site.path().join(".ddev")).expect("remove regular DDEV directory");
    fs::write(
        external_ddev.path().join("config.yaml"),
        "type: wordpress\n",
    )
    .expect("external DDEV config");
    symlink(external_ddev.path(), site.path().join(".ddev")).expect("source DDEV symlink");
    let generated = tempfile::tempdir().expect("generated root");
    let fake_state = tempfile::tempdir().expect("fake DDEV state");
    let fake_bin = support::fake_ddev_directory(&fake_state.path().join("running"));

    let output = run_cli_with_path_and_vars(
        &repository,
        &["create", "--base", "HEAD", "unsafe"],
        fake_bin.path(),
        &[
            (
                "SYMLINK_DDEV_SITE",
                site.path().to_str().expect("source site"),
            ),
            (
                "SYMLINK_GENERATED_ROOT",
                generated.path().to_str().expect("generated root"),
            ),
        ],
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert!(!output.status.success(), "{text}");
    assert!(text.contains("regular non-symlink .ddev directory"));
    assert!(
        !external_ddev
            .path()
            .join("docker-compose.ddev-workspaces.yaml")
            .exists()
    );
}

#[test]
fn partial_external_creation_without_an_app_root_can_be_removed() {
    let (site, repository) = external_repository("PARTIAL_DDEV_SITE", "PARTIAL_GENERATED_ROOT");
    let generated = tempfile::tempdir().expect("generated root");
    let created = support::run_cli(
        &repository,
        &["create", "--source-only", "--base", "HEAD", "partial"],
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    let record_path = repository.join(".git/ddev-workspaces/workspaces/partial.toml");
    let app_root = fs::canonicalize(generated.path())
        .expect("canonical generated root")
        .join("fixture/partial");
    let mut record = fs::read_to_string(&record_path).expect("ownership record");
    record = record.replace("source_only = true", "source_only = false");
    record = record.replace("external_ddev_site = false", "external_ddev_site = true");
    record.push_str(&format!(
        "ddev_app_root = {:?}\n",
        app_root.display().to_string()
    ));
    fs::write(&record_path, record).expect("partial ownership record");

    let fake_state = tempfile::tempdir().expect("fake DDEV state");
    let fake_bin = support::fake_ddev_directory(&fake_state.path().join("running"));
    let removed = run_cli_with_path_and_vars(
        &repository,
        &["remove", "--confirm", "partial", "partial"],
        fake_bin.path(),
        &[
            (
                "PARTIAL_DDEV_SITE",
                site.path().to_str().expect("source site"),
            ),
            (
                "PARTIAL_GENERATED_ROOT",
                generated.path().to_str().expect("generated root"),
            ),
        ],
    );
    let text = format!("{}{}", stdout(&removed), support::stderr(&removed));

    assert!(removed.status.success(), "{text}");
    assert!(!repository.join(".worktrees/partial").exists());
    assert!(!record_path.exists());
}

#[test]
fn removal_refuses_external_mode_drift_without_orphaning_owned_state() {
    let (site, repository) = external_repository("DRIFT_DDEV_SITE", "DRIFT_GENERATED_ROOT");
    let generated = tempfile::tempdir().expect("generated root");
    let generated_site = fs::canonicalize(generated.path())
        .expect("canonical generated root")
        .join("fixture/drift");
    let fake_state = tempfile::tempdir().expect("fake DDEV state");
    let state = fake_state.path().join("running");
    let fake_bin = support::fake_ddev_directory(&state);
    let variables = [
        ("DDEV_FAKE_NAME", "dw-fixture--drift"),
        (
            "DRIFT_DDEV_SITE",
            site.path().to_str().expect("source site"),
        ),
        (
            "DRIFT_GENERATED_ROOT",
            generated.path().to_str().expect("generated root"),
        ),
    ];
    let created = run_cli_with_path_and_vars(
        &repository,
        &["create", "--base", "HEAD", "drift"],
        fake_bin.path(),
        &variables,
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    fs::write(
        repository.join(".ddev-workspaces.toml"),
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    )
    .expect("plain DDEV configuration");
    let removed = run_cli_with_path_and_vars(
        &repository,
        &["remove", "--confirm", "drift", "drift"],
        fake_bin.path(),
        &variables,
    );
    let text = format!("{}{}", stdout(&removed), support::stderr(&removed));
    let record_path = repository.join(".git/ddev-workspaces/workspaces/drift.toml");

    assert!(!removed.status.success(), "{text}");
    assert!(text.contains("current DDEV mode differs from creation provenance"));
    assert!(generated_site.exists());
    assert!(repository.join(".worktrees/drift").exists());
    assert!(record_path.exists());
}

#[test]
fn doctor_does_not_let_ddev_prune_the_real_project_registry() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    );
    commit(repository.path(), "add DDEV fixture");

    let global_home = tempfile::tempdir().expect("fake DDEV global home");
    let global_dir = global_home.path().join("ddev");
    fs::create_dir(&global_dir).expect("fake DDEV global directory");
    let registry = global_dir.join("project_list.yaml");
    fs::write(&registry, "stale registration must remain\n").expect("fake DDEV registry");
    let fake_bin = support::fake_pruning_ddev_directory();

    let output = run_cli_with_path_and_vars(
        repository.path(),
        &["doctor"],
        fake_bin.path(),
        &[
            (
                "DDEV_XDG_CONFIG_HOME",
                global_home.path().to_str().expect("global home path"),
            ),
            (
                "DDEV_FAKE_GLOBAL_DIR",
                global_dir.to_str().expect("global directory path"),
            ),
        ],
    );

    assert!(output.status.success(), "{}", support::stderr(&output));
    assert!(
        stdout(&output).contains("stale DDEV registration points to missing path /missing/control")
    );
    assert_eq!(
        fs::read_to_string(registry).expect("original registry remains"),
        "stale registration must remain\n"
    );
}

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
    assert!(
        calls
            .lines()
            .any(|line| line == "stop --unlist dw-fixture--task-1")
    );
    assert!(!calls.lines().any(|line| line.contains("delete")));
}

#[test]
fn doctor_keeps_full_workspace_runtime_and_ddev_checks() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    );
    support::write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add full doctor fixture");

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let state = fake_state.path().join("running");
    let fake_bin = support::fake_ddev_directory(&state);
    let variables = [("DDEV_FAKE_NAME", "dw-fixture--doctor-full")];
    let created = run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--base", "HEAD", "doctor-full"],
        fake_bin.path(),
        &variables,
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    let workspace = repository.path().join(".worktrees/doctor-full");
    let workspace_value = workspace.to_str().expect("workspace path");
    let doctor = run_cli_with_path_and_vars(
        repository.path(),
        &["doctor", workspace_value],
        fake_bin.path(),
        &variables,
    );
    let text = format!("{}{}", stdout(&doctor), support::stderr(&doctor));

    assert!(doctor.status.success(), "{text}");
    assert!(text.contains("Runtime: READY"), "{text}");
    assert!(text.contains("DDEV: dw-fixture--doctor-full"), "{text}");
    assert!(!text.contains("skipped by --source-only"), "{text}");
}

#[test]
fn data_deletion_dry_run_accepts_an_exact_owned_ddev_without_confirmation() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    );
    support::write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add data deletion dry-run fixture");

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let state = fake_state.path().join("running");
    let fake_log = fake_state.path().join("calls.log");
    let fake_bin = support::fake_ddev_directory(&state);
    let variables = [
        ("DDEV_FAKE_NAME", "dw-fixture--data-dry-run"),
        ("DDEV_FAKE_LOG", fake_log.to_str().expect("fake log path")),
    ];
    let created = run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--base", "HEAD", "data-dry-run"],
        fake_bin.path(),
        &variables,
    );
    assert!(created.status.success(), "{}", support::stderr(&created));

    let output = run_cli_with_path_and_vars(
        repository.path(),
        &["remove", "--dry-run", "--delete-ddev-data", "data-dry-run"],
        fake_bin.path(),
        &variables,
    );
    let text = format!("{}{}", stdout(&output), support::stderr(&output));

    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("READY — removal preflight complete"),
        "{text}"
    );
    assert!(state.exists());
    assert!(repository.path().join(".worktrees/data-dry-run").exists());
    let calls = fs::read_to_string(fake_log).expect("fake DDEV call log");
    assert!(!calls.lines().any(|line| line.starts_with("stop ")));
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
fn removal_uses_creation_ddev_provenance_after_configuration_changes() {
    let repository = init_repo();
    support::write_tracked_file(
        repository.path(),
        ".ddev-workspaces.toml",
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n\n[ddev]\napp_root = '.'\n",
    );
    support::write_tracked_file(repository.path(), ".ddev/config.yaml", "name: fixture\n");
    commit(repository.path(), "add DDEV provenance fixture");

    let fake_state = tempfile::tempdir().expect("fake DDEV state directory");
    let state = fake_state.path().join("running");
    let fake_bin = support::fake_ddev_directory(&state);
    let variables = [("DDEV_FAKE_NAME", "dw-fixture--provenance")];
    let created = run_cli_with_path_and_vars(
        repository.path(),
        &["create", "--base", "HEAD", "provenance"],
        fake_bin.path(),
        &variables,
    );
    assert!(created.status.success(), "{}", support::stderr(&created));
    assert!(state.exists());

    fs::write(
        repository.path().join(".ddev-workspaces.toml"),
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    )
    .expect("remove current DDEV configuration");
    commit(repository.path(), "remove current DDEV configuration");

    let listed =
        run_cli_with_path_and_vars(repository.path(), &["list"], fake_bin.path(), &variables);
    let listed_text = format!("{}{}", stdout(&listed), support::stderr(&listed));
    assert_eq!(listed.status.code(), Some(1), "{listed_text}");
    assert!(
        listed_text.contains("current DDEV app root differs from creation provenance"),
        "{listed_text}"
    );

    let removed = run_cli_with_path_and_vars(
        repository.path(),
        &["remove", "--confirm", "provenance", "provenance"],
        fake_bin.path(),
        &variables,
    );
    assert!(removed.status.success(), "{}", support::stderr(&removed));
    assert!(!state.exists());
    assert!(!repository.path().join(".worktrees/provenance").exists());
    assert!(
        !repository
            .path()
            .join(".git/ddev-workspaces/workspaces/provenance.toml")
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
