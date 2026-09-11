mod support;

use std::fs;

use tempfile::tempdir;

use support::{
    commit, fake_ddev_directory, init_repo, run_git, run_with_fake_ddev, run_with_fake_ddev_at,
    stderr, stdout, write_laravel_config,
};

#[test]
fn host_init_create_and_remove_keep_one_shared_host_identity() {
    let repository = init_repo();
    write_laravel_config(repository.path(), "copy");
    commit(repository.path(), "add workspace configuration");
    let fake = fake_ddev_directory();
    fs::create_dir_all(repository.path().join(".ddev-workspaces")).unwrap();
    fs::write(
        repository
            .path()
            .join(".ddev-workspaces/.fake-mutagen-enabled"),
        "enabled\n",
    )
    .unwrap();

    let initialized = run_with_fake_ddev(repository.path(), &["host", "init"], fake.path());
    assert!(initialized.status.success(), "{}", stdout(&initialized));
    assert!(
        repository
            .path()
            .join(".ddev/config.ddev-workspaces.yaml")
            .is_file()
    );
    assert!(
        repository
            .path()
            .join(".ddev/web-entrypoint.d/ddev-workspaces.sh")
            .is_file()
    );
    let initialized_again = run_with_fake_ddev(repository.path(), &["host", "init"], fake.path());
    assert!(
        initialized_again.status.success(),
        "{}\n{}",
        stdout(&initialized_again),
        stderr(&initialized_again)
    );
    assert!(stdout(&initialized_again).contains("already current"));
    let commands =
        fs::read_to_string(repository.path().join(".ddev-workspaces/.fake-commands")).unwrap();
    assert_eq!(
        commands.lines().filter(|line| *line == "restart").count(),
        1
    );

    let created = run_with_fake_ddev(
        repository.path(),
        &["create", "--base", "HEAD", "--fresh", "feature/api"],
        fake.path(),
    );
    let created_text = stdout(&created);
    assert!(
        created.status.success(),
        "{created_text}\n{}",
        stderr(&created)
    );
    assert!(created_text.contains("https://fixture-feature-api.fixture.ddev.site"));
    let commands =
        fs::read_to_string(repository.path().join(".ddev-workspaces/.fake-commands")).unwrap();
    assert!(commands.lines().any(|line| line == "mutagen"));
    let database_contents = fs::read_to_string(
        repository
            .path()
            .join(".ddev-workspaces/.fake-database-contents"),
    )
    .unwrap();
    assert!(database_contents.contains("dw_fixture_feature_api=empty"));
    assert!(!database_contents.contains("dw_fixture_feature_api=source-sentinel"));
    let diagnosed = run_with_fake_ddev(repository.path(), &["doctor"], fake.path());
    assert!(
        diagnosed.status.success(),
        "{}\n{}",
        stdout(&diagnosed),
        stderr(&diagnosed)
    );
    assert!(stdout(&diagnosed).contains("READY"));
    let workspace = repository
        .path()
        .join(".ddev-workspaces/worktrees/fixture-feature-api");
    assert!(workspace.join("public/index.php").is_file());
    let link = repository
        .path()
        .join(".ddev-workspaces/links/fixture-feature-api");
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(workspace.join("public")).unwrap()
    );
    fs::create_dir_all(workspace.join("storage/logs")).unwrap();
    fs::write(
        workspace.join("storage/logs/laravel.log"),
        "workspace log\n",
    )
    .unwrap();

    let listed = run_with_fake_ddev(repository.path(), &["list"], fake.path());
    assert!(listed.status.success(), "{}", stdout(&listed));
    assert!(stdout(&listed).contains("feature/api  ready"));

    let removed = run_with_fake_ddev(
        repository.path(),
        &["remove", "--yes", "feature/api"],
        fake.path(),
    );
    assert!(removed.status.success(), "{}", stdout(&removed));
    assert!(!workspace.exists());
    assert!(!link.exists());
    let database_contents = fs::read_to_string(
        repository
            .path()
            .join(".ddev-workspaces/.fake-database-contents"),
    )
    .unwrap();
    assert!(!database_contents.contains("dw_fixture_feature_api="));
    assert!(
        !repository
            .path()
            .join(".ddev-workspaces/workspaces/fixture-feature-api.toml")
            .exists()
    );
    assert!(
        run_git(
            repository.path(),
            &["show-ref", "--verify", "--quiet", "refs/heads/feature/api"],
        )
        .status
        .success()
    );
}

#[test]
fn copied_database_is_retained_until_explicitly_deleted() {
    let repository = init_repo();
    write_laravel_config(repository.path(), "copy");
    commit(repository.path(), "add copy workspace configuration");
    let fake = fake_ddev_directory();
    assert!(
        run_with_fake_ddev(repository.path(), &["host", "init"], fake.path())
            .status
            .success()
    );
    let created = run_with_fake_ddev(
        repository.path(),
        &["create", "--base", "HEAD", "copy-me"],
        fake.path(),
    );
    assert!(
        created.status.success(),
        "{}\n{}",
        stdout(&created),
        stderr(&created)
    );
    let database_contents = fs::read_to_string(
        repository
            .path()
            .join(".ddev-workspaces/.fake-database-contents"),
    )
    .unwrap();
    assert!(database_contents.contains("dw_fixture_copy_me=source-sentinel"));

    let removed = run_with_fake_ddev(
        repository.path(),
        &["remove", "--yes", "copy-me"],
        fake.path(),
    );
    assert!(removed.status.success(), "{}", stdout(&removed));
    let listed = run_with_fake_ddev(repository.path(), &["list"], fake.path());
    assert!(stdout(&listed).contains("copy-me  retained-data"));

    let deleted = run_with_fake_ddev(
        repository.path(),
        &["remove", "--yes", "--delete-database", "copy-me"],
        fake.path(),
    );
    assert!(deleted.status.success(), "{}", stdout(&deleted));
    let database_contents = fs::read_to_string(
        repository
            .path()
            .join(".ddev-workspaces/.fake-database-contents"),
    )
    .unwrap();
    assert!(!database_contents.contains("dw_fixture_copy_me="));
    assert!(
        !stdout(&run_with_fake_ddev(
            repository.path(),
            &["list"],
            fake.path()
        ))
        .contains("copy-me")
    );
}

#[test]
fn failed_provisioning_leaves_an_error_record_that_can_be_removed() {
    let repository = init_repo();
    write_laravel_config(repository.path(), "fresh");
    fs::write(
        repository.path().join(".ddev-workspaces.toml"),
        "version = 2\nproject_id = 'fixture'\nworktree_root = '.ddev-workspaces/worktrees'\n\n[runtime]\nadapter = 'laravel'\nhost = '.'\ndocroot = 'public'\n\n[database]\ndefault = 'fresh'\n\n[[provision]]\nlabel = 'intentional failure'\nargv = ['false']\n",
    )
    .unwrap();
    commit(repository.path(), "add failing provision");
    let fake = fake_ddev_directory();
    assert!(
        run_with_fake_ddev(repository.path(), &["host", "init"], fake.path())
            .status
            .success()
    );
    let failed = run_with_fake_ddev(
        repository.path(),
        &["create", "--base", "HEAD", "will-fail"],
        fake.path(),
    );
    assert!(!failed.status.success());
    let listed = run_with_fake_ddev(repository.path(), &["list"], fake.path());
    let listed_text = stdout(&listed);
    assert!(listed_text.contains("will-fail  error"), "{listed_text}");
    assert!(
        fs::symlink_metadata(
            repository
                .path()
                .join(".ddev-workspaces/links/fixture-will-fail")
        )
        .is_err(),
        "failed creation must not publish a route"
    );
    assert!(
        fs::symlink_metadata(
            repository
                .path()
                .join(".ddev-workspaces/views/fixture-will-fail")
        )
        .is_err(),
        "Laravel failure must not leave a generated view"
    );
    let removed = run_with_fake_ddev(
        repository.path(),
        &["remove", "--yes", "--delete-database", "will-fail"],
        fake.path(),
    );
    assert!(removed.status.success(), "{}", stdout(&removed));
}

#[test]
fn incomplete_database_copy_refuses_automatic_database_deletion_before_teardown() {
    let repository = init_repo();
    write_laravel_config(repository.path(), "copy");
    commit(repository.path(), "add incomplete copy configuration");
    let fake = fake_ddev_directory();
    assert!(
        run_with_fake_ddev(repository.path(), &["host", "init"], fake.path())
            .status
            .success()
    );
    let state = repository.path().join(".ddev-workspaces");
    fs::write(state.join(".fake-import-fails"), "fail\n").unwrap();
    fs::write(state.join(".fake-drop-fails"), "fail\n").unwrap();

    let failed = run_with_fake_ddev(
        repository.path(),
        &["create", "--base", "HEAD", "incomplete-copy"],
        fake.path(),
    );
    assert!(!failed.status.success());
    let workspace = repository
        .path()
        .join(".ddev-workspaces/worktrees/fixture-incomplete-copy");
    assert!(workspace.is_dir());
    assert!(
        fs::read_to_string(state.join(".fake-databases"))
            .unwrap()
            .lines()
            .any(|line| line == "dw_fixture_incomplete_copy")
    );

    let refused = run_with_fake_ddev(
        repository.path(),
        &["remove", "--yes", "--delete-database", "incomplete-copy"],
        fake.path(),
    );
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("ownership was not durably completed"));
    assert!(workspace.is_dir());
    assert!(
        stdout(&run_with_fake_ddev(
            repository.path(),
            &["list"],
            fake.path(),
        ))
        .contains("incomplete-copy  error")
    );

    let databases = fs::read_to_string(state.join(".fake-databases")).unwrap();
    fs::write(
        state.join(".fake-databases"),
        databases
            .lines()
            .filter(|line| *line != "dw_fixture_incomplete_copy")
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let contents = fs::read_to_string(state.join(".fake-database-contents")).unwrap();
    fs::write(
        state.join(".fake-database-contents"),
        contents
            .lines()
            .filter(|line| !line.starts_with("dw_fixture_incomplete_copy="))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    fs::remove_file(state.join(".fake-import-fails")).unwrap();
    fs::remove_file(state.join(".fake-drop-fails")).unwrap();

    let removed = run_with_fake_ddev(
        repository.path(),
        &["remove", "--yes", "--delete-database", "incomplete-copy"],
        fake.path(),
    );
    assert!(removed.status.success(), "{}", stdout(&removed));
    assert!(!workspace.exists());
}

#[test]
fn failed_host_restart_leaves_a_retryable_marker() {
    let repository = init_repo();
    write_laravel_config(repository.path(), "fresh");
    commit(repository.path(), "add workspace configuration");
    let fake = fake_ddev_directory();
    fs::create_dir_all(repository.path().join(".ddev-workspaces")).unwrap();
    fs::write(
        repository
            .path()
            .join(".ddev-workspaces/.fake-restart-fails"),
        "fail\n",
    )
    .unwrap();

    let failed = run_with_fake_ddev(repository.path(), &["host", "init"], fake.path());
    assert!(!failed.status.success());
    assert!(
        repository
            .path()
            .join(".ddev-workspaces/.restart-required")
            .is_file()
    );

    let blocked = run_with_fake_ddev(
        repository.path(),
        &["create", "--base", "HEAD", "--fresh", "pending-restart"],
        fake.path(),
    );
    assert!(!blocked.status.success());
    assert!(stderr(&blocked).contains("routing restart is pending"));

    fs::remove_file(
        repository
            .path()
            .join(".ddev-workspaces/.fake-restart-fails"),
    )
    .unwrap();
    let retried = run_with_fake_ddev(repository.path(), &["host", "init"], fake.path());
    assert!(retried.status.success(), "{}", stdout(&retried));
    assert!(
        !repository
            .path()
            .join(".ddev-workspaces/.restart-required")
            .exists()
    );
    let commands =
        fs::read_to_string(repository.path().join(".ddev-workspaces/.fake-commands")).unwrap();
    assert_eq!(
        commands.lines().filter(|line| *line == "restart").count(),
        2
    );
}

#[test]
fn stopped_host_fails_before_host_asset_mutation() {
    let repository = init_repo();
    write_laravel_config(repository.path(), "fresh");
    commit(repository.path(), "add workspace configuration");
    let fake = fake_ddev_directory();
    fs::create_dir_all(repository.path().join(".ddev-workspaces")).unwrap();
    fs::write(
        repository.path().join(".ddev-workspaces/.fake-status"),
        "stopped\n",
    )
    .unwrap();

    let failed = run_with_fake_ddev(repository.path(), &["host", "init"], fake.path());
    assert!(!failed.status.success());
    assert!(stderr(&failed).contains("not running"));
    assert!(
        !repository
            .path()
            .join(".ddev/config.ddev-workspaces.yaml")
            .exists()
    );
    assert!(
        !repository
            .path()
            .join(".ddev-workspaces/.restart-required")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn removal_refuses_a_tampered_route_and_can_resume_after_restoration() {
    use std::os::unix::fs::symlink;

    let repository = init_repo();
    write_laravel_config(repository.path(), "fresh");
    commit(repository.path(), "add workspace configuration");
    let fake = fake_ddev_directory();
    assert!(
        run_with_fake_ddev(repository.path(), &["host", "init"], fake.path())
            .status
            .success()
    );
    let created = run_with_fake_ddev(
        repository.path(),
        &["create", "--base", "HEAD", "--fresh", "tampered"],
        fake.path(),
    );
    assert!(created.status.success(), "{}", stdout(&created));

    let workspace = repository
        .path()
        .join(".ddev-workspaces/worktrees/fixture-tampered");
    let link = repository
        .path()
        .join(".ddev-workspaces/links/fixture-tampered");
    let foreign_target = repository.path().join("foreign-route-target");
    fs::create_dir(&foreign_target).unwrap();
    fs::remove_file(&link).unwrap();
    symlink(&foreign_target, &link).unwrap();

    let failed = run_with_fake_ddev(
        repository.path(),
        &["remove", "--yes", "--delete-database", "tampered"],
        fake.path(),
    );
    assert!(!failed.status.success());
    assert!(stderr(&failed).contains("points to"));
    let listed = run_with_fake_ddev(repository.path(), &["list"], fake.path());
    assert!(stdout(&listed).contains("tampered  removing"));

    fs::remove_file(&link).unwrap();
    symlink(workspace.join("public"), &link).unwrap();
    let removed = run_with_fake_ddev(
        repository.path(),
        &["remove", "--yes", "--delete-database", "tampered"],
        fake.path(),
    );
    assert!(removed.status.success(), "{}", stdout(&removed));
    assert!(!workspace.exists());
}

#[test]
fn nested_wordpress_component_lifecycle_uses_distinct_generated_views() {
    let parent = tempdir().unwrap();
    let host = parent.path();
    let repository = host.join("wp-content/plugins/example");
    fs::create_dir_all(&repository).unwrap();
    fs::create_dir_all(host.join(".ddev")).unwrap();
    fs::write(
        host.join(".ddev/config.yaml"),
        "name: fixture\ntype: php\ndocroot: .\n",
    )
    .unwrap();
    for path in [
        "wp-content/plugins/other",
        "wp-content/themes/default",
        "wp-content/uploads",
        "wp-content/cache",
        "wp-content/upgrade",
        "wp-content/languages",
        "wp-content/mu-cache",
        "wp-content/mu-plugins",
        "wp-admin",
        "wp-includes",
    ] {
        fs::create_dir_all(host.join(path)).unwrap();
    }
    for path in [
        "index.php",
        "wp-content/index.php",
        "wp-admin/index.php",
        "wp-includes/version.php",
    ] {
        fs::write(host.join(path), "<?php\n").unwrap();
    }
    fs::write(
        host.join("wp-config.php"),
        "<?php\ndefine(\"DB_NAME\", \"source_db\");\ndefine(\"DB_USER\", \"root\");\ndefine(\"DB_PASSWORD\", \"root\");\ndefine(\"DB_HOST\", \"db\");\ndefine(\"WP_HOME\", \"https://source.example\");\ndefine(\"WP_SITEURL\", \"https://source.example\");\n",
    )
    .unwrap();
    fs::write(
        repository.join(".ddev-workspaces.toml"),
        "version = 2\nproject_id = 'fixture-plugin'\nworktree_root = '../../../.ddev-workspaces/worktrees'\n\n[runtime]\nadapter = 'wordpress-component'\nhost = '../../../'\ncomponent_kind = 'plugin'\ncomponent_path = 'wp-content/plugins/example'\n\n[database]\ndefault = 'fresh'\n",
    )
    .unwrap();
    fs::write(repository.join("plugin.php"), "<?php\n").unwrap();
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
            &["config", "user.email", "fixture@example.test"],
        )
        .status
        .success()
    );
    commit(&repository, "initial WordPress component");

    let fake = fake_ddev_directory();
    let initialized = run_with_fake_ddev_at(&repository, &["host", "init"], fake.path(), host);
    assert!(initialized.status.success(), "{}", stdout(&initialized));
    for name in ["feature/one", "feature/two"] {
        let created = run_with_fake_ddev_at(
            &repository,
            &["create", "--base", "HEAD", "--fresh", name],
            fake.path(),
            host,
        );
        assert!(created.status.success(), "{}", stdout(&created));
    }

    let first_key = "fixture-plugin-feature-one";
    let second_key = "fixture-plugin-feature-two";
    let first_workspace = host.join(".ddev-workspaces/worktrees").join(first_key);
    let second_workspace = host.join(".ddev-workspaces/worktrees").join(second_key);
    let first_view = host.join(".ddev-workspaces/views").join(first_key);
    let second_view = host.join(".ddev-workspaces/views").join(second_key);
    let first_link = host.join(".ddev-workspaces/links").join(first_key);
    let second_link = host.join(".ddev-workspaces/links").join(second_key);
    assert_eq!(
        fs::canonicalize(first_view.join("wp-content/plugins/example")).unwrap(),
        fs::canonicalize(&first_workspace).unwrap()
    );
    assert_eq!(
        fs::canonicalize(second_view.join("wp-content/plugins/example")).unwrap(),
        fs::canonicalize(&second_workspace).unwrap()
    );
    assert_eq!(
        fs::canonicalize(&first_link).unwrap(),
        fs::canonicalize(&first_view).unwrap()
    );
    assert_eq!(
        fs::canonicalize(&second_link).unwrap(),
        fs::canonicalize(&second_view).unwrap()
    );
    assert!(!first_workspace.join("wp-admin").exists());
    assert!(first_view.join("wp-content/uploads").is_dir());
    assert!(second_view.join("wp-content/uploads").is_dir());
    assert!(!first_view.join("wp-content/uploads").is_symlink());
    assert!(!second_view.join("wp-content/uploads").is_symlink());
    let first_config = fs::read_to_string(first_view.join("wp-config.php")).unwrap();
    let second_config = fs::read_to_string(second_view.join("wp-config.php")).unwrap();
    assert!(first_config.contains("https://fixture-plugin-feature-one.fixture.ddev.site"));
    assert!(second_config.contains("https://fixture-plugin-feature-two.fixture.ddev.site"));
    assert!(!first_config.contains("source_db"));
    let diagnosed = run_with_fake_ddev_at(&repository, &["doctor"], fake.path(), host);
    assert!(diagnosed.status.success(), "{}", stdout(&diagnosed));

    let removed = run_with_fake_ddev_at(
        &repository,
        &["remove", "--yes", "--delete-database", "feature/one"],
        fake.path(),
        host,
    );
    assert!(removed.status.success(), "{}", stdout(&removed));
    assert!(!first_view.exists());
    assert!(!first_link.exists());
    assert!(second_view.exists());
    assert!(second_link.exists());
    let removed = run_with_fake_ddev_at(
        &repository,
        &["remove", "--yes", "--delete-database", "feature/two"],
        fake.path(),
        host,
    );
    assert!(removed.status.success(), "{}", stdout(&removed));
    assert!(!second_view.exists());
}
