use super::*;
use crate::command::{CommandOutput, CommandRunner};
use std::cell::RefCell;
use tempfile::tempdir;

#[derive(Default)]
struct IgnoreRunner {
    ignored: bool,
    unignored_paths: Vec<String>,
    requests: RefCell<Vec<CommandRequest>>,
}

impl CommandRunner for IgnoreRunner {
    fn run(&mut self, request: &CommandRequest) -> ToolResult<CommandOutput> {
        self.requests.borrow_mut().push(request.clone());
        let path = request.args.last().map(String::as_str).unwrap_or_default();
        let ignored = self.ignored
            && !self
                .unignored_paths
                .iter()
                .any(|candidate| candidate == path);
        Ok(CommandOutput {
            status: if ignored { 0 } else { 1 },
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn valid_config() -> ProjectConfig {
    ProjectConfig {
        version: 2,
        project_id: "fixture".to_owned(),
        worktree_root: ".worktrees".to_owned(),
        runtime: RuntimeConfig {
            adapter: "laravel".to_owned(),
            host: ".".to_owned(),
            docroot: Some("public".to_owned()),
            component_kind: None,
            component_path: None,
        },
        database: DatabaseConfig::default(),
        environment: None,
        carry: Vec::new(),
        provision: Vec::new(),
        checks: Vec::new(),
        writable: Vec::new(),
    }
}

#[test]
fn unsupported_versions_and_unknown_fields_are_rejected() {
    let directory = tempdir().expect("repository");
    fs::create_dir(directory.path().join(".ddev")).expect("DDEV directory");
    fs::write(
        directory.path().join(".ddev/config.yaml"),
        "name: fixture\n",
    )
    .expect("DDEV config");
    fs::write(
        directory.path().join(CONFIG_FILE),
        "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
    )
    .expect("config");
    let error = ProjectConfig::load(directory.path()).unwrap_err();
    assert!(error.to_string().contains("invalid configuration"));

    let error = toml::from_str::<ProjectConfig>(
        "version = 2\nproject_id = 'fixture'\nworktree_root = '.worktrees'\nfuture = true\n[runtime]\nadapter = 'laravel'\nhost = '.'\n",
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn resource_names_are_readable_and_long_names_get_a_stable_hash() {
    assert_eq!(
        resource_key("filebean", "feature/fix"),
        "filebean-feature-fix"
    );
    let first = resource_key("project", &"feature/".repeat(20));
    let second = resource_key("project", &"feature/".repeat(20));
    assert_eq!(first, second);
    assert!(first.len() <= 52);
    assert!(first.chars().any(|character| character.is_ascii_digit()));
}

#[test]
fn workspace_names_allow_branch_slashes_but_reject_path_or_option_escapes() {
    assert!(validate_workspace_name("feature/fix-1").is_ok());
    assert!(validate_workspace_name("-bad").is_err());
    assert!(validate_workspace_name("feature//fix").is_err());
    assert!(validate_workspace_name("feature..fix").is_err());
    assert!(validate_workspace_name("feature/a:b").is_err());
    assert!(validate_workspace_name("feature/.hidden").is_err());
    assert!(validate_workspace_name("feature/a.lock").is_err());
    assert!(validate_workspace_name("feature/a.").is_err());
}

#[test]
fn minimal_laravel_structure_is_valid_when_the_worktree_root_is_ignored() {
    let directory = tempdir().expect("repository");
    fs::create_dir(directory.path().join(".ddev")).expect("DDEV directory");
    fs::write(
        directory.path().join(".ddev/config.yaml"),
        "name: fixture\n",
    )
    .expect("DDEV config");
    let mut runner = IgnoreRunner {
        ignored: true,
        ..Default::default()
    };
    valid_config()
        .validate_structure(directory.path(), &mut runner)
        .expect("structure should validate");
    assert_eq!(runner.requests.borrow().len(), 1);
}

#[test]
fn unsupported_adapters_and_database_modes_fail_closed() {
    let directory = tempdir().expect("repository");
    fs::create_dir(directory.path().join(".ddev")).expect("DDEV directory");
    fs::write(
        directory.path().join(".ddev/config.yaml"),
        "name: fixture\n",
    )
    .expect("DDEV config");
    let mut runner = IgnoreRunner {
        ignored: true,
        ..Default::default()
    };
    let mut config = valid_config();
    config.runtime.adapter = "apache".to_owned();
    assert!(
        config
            .validate_structure(directory.path(), &mut runner)
            .is_err()
    );
    let mut config = valid_config();
    config.database.default = "postgres".to_owned();
    assert!(
        config
            .validate_structure(directory.path(), &mut runner)
            .is_err()
    );
}

#[test]
fn environment_replacements_are_limited_to_workspace_owned_keys() {
    let directory = tempdir().expect("repository");
    fs::create_dir(directory.path().join(".ddev")).expect("DDEV directory");
    fs::write(
        directory.path().join(".ddev/config.yaml"),
        "name: fixture\n",
    )
    .expect("DDEV config");
    fs::write(directory.path().join(".env.example"), "APP_KEY=fixture\n")
        .expect("environment source");
    let mut runner = IgnoreRunner {
        ignored: true,
        ..Default::default()
    };
    let mut config = valid_config();
    config.environment = Some(EnvironmentConfig {
        source: ".env.example".to_owned(),
        destination: None,
        replace: vec!["APP_KEY".to_owned()],
    });
    let error = config
        .validate_structure(directory.path(), &mut runner)
        .unwrap_err();
    assert!(error.to_string().contains("not workspace-owned"));
}

#[test]
fn generated_environment_and_sensitive_carry_destinations_must_be_ignored() {
    let directory = tempdir().expect("repository");
    fs::create_dir(directory.path().join(".ddev")).expect("DDEV directory");
    fs::write(
        directory.path().join(".ddev/config.yaml"),
        "name: fixture\n",
    )
    .expect("DDEV config");
    fs::write(
        directory.path().join(".env.example"),
        "APP_URL=http://main\n",
    )
    .expect("environment source");
    fs::write(directory.path().join("secret.txt"), "secret\n").expect("carry source");

    let mut environment_runner = IgnoreRunner {
        ignored: true,
        unignored_paths: vec![".env.generated".to_owned()],
        ..Default::default()
    };
    let mut environment_config = valid_config();
    environment_config.environment = Some(EnvironmentConfig {
        source: ".env.example".to_owned(),
        destination: Some(".env.generated".to_owned()),
        replace: Vec::new(),
    });
    let error = environment_config
        .validate_structure(directory.path(), &mut environment_runner)
        .unwrap_err();
    assert!(error.to_string().contains("must already be ignored"));

    let mut carry_runner = IgnoreRunner {
        ignored: true,
        unignored_paths: vec!["secret.generated".to_owned()],
        ..Default::default()
    };
    let mut carry_config = valid_config();
    carry_config.carry = vec![CarryRule {
        source: "secret.txt".to_owned(),
        destination: Some("secret.generated".to_owned()),
        required: true,
        sensitive: true,
    }];
    let error = carry_config
        .validate_structure(directory.path(), &mut carry_runner)
        .unwrap_err();
    assert!(error.to_string().contains("sensitive carry destination"));
}

#[test]
fn laravel_environment_must_replace_the_database_name() {
    let directory = tempdir().expect("repository");
    fs::create_dir(directory.path().join(".ddev")).expect("DDEV directory");
    fs::write(
        directory.path().join(".ddev/config.yaml"),
        "name: fixture\n",
    )
    .expect("DDEV config");
    fs::write(
        directory.path().join(".env.example"),
        "APP_URL=http://main\n",
    )
    .expect("environment source");
    let mut runner = IgnoreRunner {
        ignored: true,
        ..Default::default()
    };
    let mut config = valid_config();
    config.environment = Some(EnvironmentConfig {
        source: ".env.example".to_owned(),
        destination: None,
        replace: vec!["APP_URL".to_owned()],
    });
    let error = config
        .validate_structure(directory.path(), &mut runner)
        .unwrap_err();
    assert!(error.to_string().contains("must include DB_DATABASE"));
}

#[test]
fn generated_sensitive_paths_cannot_be_below_the_laravel_docroot() {
    let directory = tempdir().expect("repository");
    fs::create_dir(directory.path().join(".ddev")).expect("DDEV directory");
    fs::write(
        directory.path().join(".ddev/config.yaml"),
        "name: fixture\n",
    )
    .expect("DDEV config");
    fs::write(
        directory.path().join(".env.example"),
        "APP_URL=http://main\nDB_DATABASE=main\n",
    )
    .expect("environment source");
    let mut runner = IgnoreRunner {
        ignored: true,
        ..Default::default()
    };
    let mut config = valid_config();
    config.environment = Some(EnvironmentConfig {
        source: ".env.example".to_owned(),
        destination: Some("public/.env".to_owned()),
        replace: vec!["APP_URL".to_owned(), "DB_DATABASE".to_owned()],
    });
    let error = config
        .validate_structure(directory.path(), &mut runner)
        .unwrap_err();
    assert!(error.to_string().contains("inside Laravel runtime.docroot"));
}

#[test]
fn nested_component_worktrees_cannot_use_host_git_metadata() {
    let directory = tempdir().expect("fixture root");
    let host = directory.path().join("host");
    let repository = host.join("wp-content/plugins/example");
    fs::create_dir_all(repository.join(".git")).expect("component Git metadata");
    fs::create_dir_all(host.join(".ddev")).expect("DDEV directory");
    fs::create_dir_all(host.join(".git")).expect("host Git metadata");
    fs::write(host.join(".ddev/config.yaml"), "name: fixture\n").expect("DDEV config");
    let config = ProjectConfig {
        version: 2,
        project_id: "fixture".to_owned(),
        worktree_root: "../../../.git/workspaces".to_owned(),
        runtime: RuntimeConfig {
            adapter: "wordpress-component".to_owned(),
            host: "../../../".to_owned(),
            docroot: None,
            component_kind: Some("plugin".to_owned()),
            component_path: Some("wp-content/plugins/example".to_owned()),
        },
        database: DatabaseConfig::default(),
        environment: None,
        carry: Vec::new(),
        provision: Vec::new(),
        checks: Vec::new(),
        writable: Vec::new(),
    };
    let mut runner = IgnoreRunner {
        ignored: true,
        ..Default::default()
    };
    let error = config
        .validate_structure(&repository, &mut runner)
        .unwrap_err();
    assert!(error.to_string().contains("runtime host Git metadata"));
}
