use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::command::{CommandRequest, CommandRunner, ToolError, ToolResult};

pub const CONFIG_FILE: &str = ".ddev-workspaces.toml";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub version: u32,
    pub project_id: String,
    pub workspace_root: String,
    #[serde(default)]
    pub ddev: Option<DdevConfig>,
    #[serde(default)]
    pub files: Vec<FileRule>,
    #[serde(default)]
    pub commands: Vec<CommandRule>,
    #[serde(default)]
    pub checks: Vec<CheckRule>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DdevConfig {
    pub app_root: String,
    #[serde(default)]
    pub source_site: Option<SourceSiteConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceSiteConfig {
    pub repository_path: String,
    #[serde(default)]
    pub clone_database: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSourceSite {
    pub source_root: PathBuf,
    pub generated_root: PathBuf,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileRule {
    pub label: String,
    pub destination: String,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub source_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommandRule {
    pub label: String,
    pub cwd: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckRule {
    pub label: String,
    pub kind: String,
    pub path: String,
    #[serde(default)]
    pub key: Option<String>,
}

impl ProjectConfig {
    pub fn load(repo_root: &Path) -> ToolResult<Self> {
        let path = repo_root.join(CONFIG_FILE);
        let contents = fs::read_to_string(&path).map_err(|error| {
            ToolError::new(format!(
                "configuration is required at {}: {error}; add version, project_id, and workspace_root",
                path.display()
            ))
        })?;
        let config = toml::from_str::<Self>(&contents).map_err(|error| {
            ToolError::new(format!(
                "invalid configuration at {}: {error}; correct the named field and rerun doctor",
                path.display()
            ))
        })?;
        Ok(config)
    }

    pub fn validate<R: CommandRunner>(&self, repo_root: &Path, runner: &mut R) -> ToolResult<()> {
        self.validate_structure(repo_root, runner)?;
        if let Some(source_site) = self
            .ddev
            .as_ref()
            .and_then(|ddev| ddev.source_site.as_ref())
        {
            resolve_source_site(repo_root, source_site, false)?;
        }
        Ok(())
    }

    pub fn validate_structure<R: CommandRunner>(
        &self,
        repo_root: &Path,
        runner: &mut R,
    ) -> ToolResult<()> {
        if self.version != 1 {
            return Err(ToolError::new(format!(
                "configuration field `version` must be integer 1, found {}; update {}",
                self.version, CONFIG_FILE
            )));
        }
        validate_name("project_id", &self.project_id)?;
        validate_repository_relative("workspace_root", &self.workspace_root)?;

        let workspace_root = safe_join(repo_root, &self.workspace_root)?;
        let ignore_probe = workspace_root.join(".ddev-workspaces-ignore-probe");
        if !is_ignored(repo_root, &ignore_probe, runner)? {
            return Err(ToolError::new(format!(
                "configuration field `workspace_root` must already be ignored by Git: {}; add a local ignore rule manually",
                workspace_root.display()
            )));
        }

        if let Some(ddev) = &self.ddev {
            validate_repository_relative("ddev.app_root", &ddev.app_root)?;
            let app_root = safe_join(repo_root, &ddev.app_root)?;
            if app_root.join(".ddev").is_symlink() {
                return Err(ToolError::new(
                    "configuration field `ddev.app_root` resolves through a symlinked .ddev directory",
                ));
            }
            if let Some(source_site) = &ddev.source_site {
                validate_repository_relative(
                    "ddev.source_site.repository_path",
                    &source_site.repository_path,
                )?;
            }
        }

        let mut destinations = Vec::new();
        for file in &self.files {
            if file.label.trim().is_empty() {
                return Err(ToolError::new(
                    "configuration field `files[].label` must not be empty",
                ));
            }
            validate_workspace_relative("files[].destination", &file.destination)?;
            let destination = normalize_relative_path(&file.destination);
            if destination.as_os_str().is_empty()
                || destinations.iter().any(|existing: &PathBuf| {
                    existing == &destination
                        || existing.starts_with(&destination)
                        || destination.starts_with(existing)
                })
            {
                return Err(ToolError::new(format!(
                    "file rule `{}` conflicts with destination `{}`; file destinations must be distinct files",
                    file.label, file.destination
                )));
            }
            destinations.push(destination);
            match (&file.template, &file.source_env) {
                (Some(_), None) | (None, Some(_)) => {}
                (Some(_), Some(_)) => {
                    return Err(ToolError::new(format!(
                        "file rule `{}` must set exactly one of `template` or `source_env`",
                        file.label
                    )));
                }
                (None, None) => {
                    return Err(ToolError::new(format!(
                        "file rule `{}` must set exactly one of `template` or `source_env`",
                        file.label
                    )));
                }
            }
            if let Some(template) = &file.template {
                validate_repository_relative("files[].template", template)?;
            }
            if let Some(source_env) = &file.source_env {
                validate_environment_name(source_env)?;
            }
        }

        for command in &self.commands {
            if command.label.trim().is_empty() {
                return Err(ToolError::new(
                    "configuration field `commands[].label` must not be empty",
                ));
            }
            validate_repository_relative("commands[].cwd", &command.cwd)?;
            if command.argv.is_empty() || command.argv.iter().any(|argument| argument.is_empty()) {
                return Err(ToolError::new(format!(
                    "command `{}` must have a non-empty argv array",
                    command.label
                )));
            }
        }

        for check in &self.checks {
            if check.label.trim().is_empty() {
                return Err(ToolError::new(
                    "configuration field `checks[].label` must not be empty",
                ));
            }
            validate_workspace_relative("checks[].path", &check.path)?;
            match check.kind.as_str() {
                "path-exists" if check.key.is_none() => {}
                "env-key" if check.key.as_deref().is_some_and(|key| !key.is_empty()) => {
                    validate_environment_name(check.key.as_deref().unwrap_or_default())?;
                }
                "path-exists" => {
                    return Err(ToolError::new(format!(
                        "check `{}` of kind `path-exists` cannot define `key`",
                        check.label
                    )));
                }
                "env-key" => {
                    return Err(ToolError::new(format!(
                        "check `{}` of kind `env-key` requires a non-empty `key`",
                        check.label
                    )));
                }
                _ => {
                    return Err(ToolError::new(format!(
                        "check `{}` has unsupported kind `{}`; use `path-exists` or `env-key`",
                        check.label, check.kind
                    )));
                }
            }
        }
        Ok(())
    }
}

pub fn source_site_root(repo_root: &Path) -> ToolResult<PathBuf> {
    let canonical_repo_root = fs::canonicalize(repo_root)?;
    for ancestor in canonical_repo_root.ancestors().skip(1) {
        let ddev_directory = ancestor.join(".ddev");
        let ddev_metadata = match fs::symlink_metadata(&ddev_directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(ToolError::new(format!(
                    "cannot inspect potential source DDEV site {}: {error}",
                    ancestor.display()
                )));
            }
        };
        if !ddev_metadata.is_dir() || ddev_metadata.file_type().is_symlink() {
            return Err(ToolError::new(format!(
                "source DDEV site {} must contain a regular non-symlink .ddev directory",
                ancestor.display()
            )));
        }
        let config_path = ddev_directory.join("config.yaml");
        let config_metadata = fs::symlink_metadata(&config_path).map_err(|error| {
            ToolError::new(format!(
                "source DDEV site root {} has no usable .ddev/config.yaml: {error}",
                ancestor.display()
            ))
        })?;
        if !config_metadata.is_file() || config_metadata.file_type().is_symlink() {
            return Err(ToolError::new(format!(
                "source DDEV site root {} must contain a regular non-symlink .ddev/config.yaml",
                ancestor.display()
            )));
        }
        return Ok(ancestor.to_path_buf());
    }
    Err(ToolError::new(format!(
        "repository {} is not contained in a DDEV site; add a regular .ddev/config.yaml to a parent directory or remove ddev.source_site",
        canonical_repo_root.display()
    )))
}

pub fn source_generated_root() -> ToolResult<PathBuf> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| ToolError::new("home directory is unavailable; HOME must be absolute"))?;
    let path = home.join(".ddev-workspaces/sites");
    let existing = nearest_existing_parent(&path);
    let canonical_existing = fs::canonicalize(&existing)?;
    let remainder = path
        .strip_prefix(&existing)
        .map_err(|_| ToolError::new("cannot resolve the default generated DDEV root"))?;
    Ok(canonical_existing.join(remainder))
}

pub fn resolve_source_site(
    repo_root: &Path,
    config: &SourceSiteConfig,
    create_generated_root: bool,
) -> ToolResult<ResolvedSourceSite> {
    let source_root = source_site_root(repo_root)?;
    let mut generated_root = source_generated_root()?;
    if create_generated_root {
        create_directory_tree(&generated_root)?;
        generated_root = fs::canonicalize(&generated_root)?;
    }
    if generated_root.starts_with(&source_root) || source_root.starts_with(&generated_root) {
        return Err(ToolError::new(format!(
            "generated DDEV root {} must be outside source DDEV site {}",
            generated_root.display(),
            source_root.display()
        )));
    }
    let declared_repository = safe_join(&source_root, &config.repository_path)?;
    let canonical_repository = fs::canonicalize(&declared_repository).map_err(|error| {
        ToolError::new(format!(
            "source DDEV site repository path {} is unavailable: {error}",
            declared_repository.display()
        ))
    })?;
    let canonical_repo_root = fs::canonicalize(repo_root)?;
    if canonical_repository != canonical_repo_root {
        return Err(ToolError::new(format!(
            "ddev.source_site.repository_path resolves to {}, not this repository {}; refusing an unproven mount",
            canonical_repository.display(),
            canonical_repo_root.display()
        )));
    }
    Ok(ResolvedSourceSite {
        source_root,
        generated_root,
    })
}

fn create_directory_tree(path: &Path) -> ToolResult<()> {
    let existing = nearest_existing_parent(path);
    let mut current = fs::canonicalize(&existing)?;
    let remainder = path.strip_prefix(&existing).map_err(|_| {
        ToolError::new(format!(
            "cannot prepare generated DDEV root {}",
            path.display()
        ))
    })?;
    for component in remainder.components() {
        let Component::Normal(part) = component else {
            return Err(ToolError::new(format!(
                "generated DDEV root {} contains an unsupported path component",
                path.display()
            )));
        };
        current.push(part);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&current)?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(ToolError::new(format!(
                        "generated DDEV root component {} is not a regular directory",
                        current.display()
                    )));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn validate_name(field: &str, value: &str) -> ToolResult<()> {
    let valid_first = value
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    let valid_last = value
        .as_bytes()
        .last()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    let valid_characters = value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if value.is_empty() || value.contains("--") || !valid_first || !valid_last || !valid_characters
    {
        return Err(ToolError::new(format!(
            "{field} `{value}` must match [a-z0-9](?:[a-z0-9-]*[a-z0-9])? and may not contain `--`"
        )));
    }
    Ok(())
}

pub fn validate_repository_relative(field: &str, value: &str) -> ToolResult<()> {
    validate_relative(field, value)
}

pub fn validate_workspace_relative(field: &str, value: &str) -> ToolResult<()> {
    validate_relative(field, value)
}

fn validate_relative(field: &str, value: &str) -> ToolResult<()> {
    let path = Path::new(value);
    if value.is_empty() || path.is_absolute() {
        return Err(ToolError::new(format!(
            "configuration field `{field}` must be a non-empty relative path"
        )));
    }
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(_) => depth += 1,
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir => {
                return Err(ToolError::new(format!(
                    "configuration field `{field}` escapes its repository/workspace root"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError::new(format!(
                    "configuration field `{field}` must be relative"
                )));
            }
        }
    }
    if depth == 0 && value != "." {
        return Err(ToolError::new(format!(
            "configuration field `{field}` must name a path"
        )));
    }
    Ok(())
}

pub fn safe_join(root: &Path, relative: &str) -> ToolResult<PathBuf> {
    validate_repository_relative("path", relative)?;
    let candidate = root.join(normalize_relative_path(relative));
    let root_canonical = fs::canonicalize(root).map_err(|error| {
        ToolError::new(format!(
            "cannot resolve repository root {}: {error}",
            root.display()
        ))
    })?;
    let existing_parent = nearest_existing_parent(&candidate);
    let parent_canonical = fs::canonicalize(&existing_parent).map_err(|error| {
        ToolError::new(format!(
            "cannot resolve path parent {}: {error}",
            existing_parent.display()
        ))
    })?;
    if !parent_canonical.starts_with(&root_canonical) {
        return Err(ToolError::new(format!(
            "path `{relative}` escapes repository root through a symlink"
        )));
    }
    Ok(candidate)
}

pub fn normalize_relative_path(relative: &str) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    normalized
}

pub fn nearest_existing_parent(path: &Path) -> PathBuf {
    let mut parent = path.to_path_buf();
    while !parent.exists() {
        let Some(next) = parent.parent() else {
            break;
        };
        if next == parent {
            break;
        }
        parent = next.to_path_buf();
    }
    parent
}

pub fn is_ignored<R: CommandRunner>(
    repo_root: &Path,
    path: &Path,
    runner: &mut R,
) -> ToolResult<bool> {
    let relative = path.strip_prefix(repo_root).map_err(|_| {
        ToolError::new(format!(
            "path {} is outside repository {}",
            path.display(),
            repo_root.display()
        ))
    })?;
    let request = CommandRequest::new(
        "git",
        [
            "-C".to_owned(),
            repo_root.display().to_string(),
            "check-ignore".to_owned(),
            "--quiet".to_owned(),
            "--".to_owned(),
            relative.display().to_string(),
        ],
    );
    let output = runner.run(&request)?;
    Ok(output.success())
}

fn validate_environment_name(value: &str) -> ToolResult<()> {
    if value.is_empty()
        || !value.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte == b'_' || byte.is_ascii_alphabetic()
            } else {
                byte == b'_' || byte.is_ascii_alphanumeric()
            }
        })
    {
        return Err(ToolError::new(format!(
            "environment variable name `{value}` is invalid; use letters, digits, and underscores"
        )));
    }
    Ok(())
}

pub fn source_from_environment(name: &str) -> ToolResult<PathBuf> {
    let value = env::var_os(name).ok_or_else(|| {
        ToolError::new(format!(
            "environment variable `{name}` is not set for the named file source; set it manually"
        ))
    })?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(ToolError::new(format!(
            "environment variable `{name}` must contain an absolute local file path"
        )));
    }
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        ToolError::new(format!(
            "named local file source `{name}` cannot be read: {error}"
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(ToolError::new(format!(
            "named local file source `{name}` must be a regular non-symlink file"
        )));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandOutput, CommandRunner};
    use std::cell::RefCell;
    use std::fs;
    use tempfile::tempdir;

    #[derive(Default)]
    struct IgnoreRunner {
        ignored: bool,
        requests: RefCell<Vec<CommandRequest>>,
    }

    impl CommandRunner for IgnoreRunner {
        fn run(&mut self, request: &CommandRequest) -> ToolResult<CommandOutput> {
            self.requests.get_mut().push(request.clone());
            Ok(CommandOutput {
                status: if self.ignored { 0 } else { 1 },
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn valid_config() -> ProjectConfig {
        ProjectConfig {
            version: 1,
            project_id: "fixture".to_owned(),
            workspace_root: ".worktrees".to_owned(),
            ddev: None,
            files: Vec::new(),
            commands: Vec::new(),
            checks: Vec::new(),
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let result = toml::from_str::<ProjectConfig>(
            "version = 1\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\nfuture = true\n",
        );

        assert!(result.is_err());
    }

    #[test]
    fn future_versions_are_rejected_with_remediation() {
        let directory = tempdir().expect("temporary directory");
        let config = directory.path().join(CONFIG_FILE);
        fs::write(
            &config,
            "version = 2\nproject_id = 'fixture'\nworkspace_root = '.worktrees'\n",
        )
        .expect("configuration should be written");
        let loaded = ProjectConfig::load(directory.path()).expect("TOML should parse");
        let mut runner = IgnoreRunner {
            ignored: true,
            ..Default::default()
        };

        let error = loaded.validate(directory.path(), &mut runner).unwrap_err();

        assert!(error.to_string().contains("integer 1"));
    }

    #[test]
    fn escaping_paths_are_rejected_before_git_ignore_is_checked() {
        let mut config = valid_config();
        config.workspace_root = "../outside".to_owned();
        let directory = tempdir().expect("temporary directory");
        let mut runner = IgnoreRunner {
            ignored: true,
            ..Default::default()
        };

        let error = config.validate(directory.path(), &mut runner).unwrap_err();

        assert!(error.to_string().contains("escapes"));
        assert!(runner.requests.borrow().is_empty());
    }

    #[test]
    fn non_ignored_workspace_roots_are_rejected() {
        let config = valid_config();
        let directory = tempdir().expect("temporary directory");
        fs::create_dir(directory.path().join(".worktrees")).expect("workspace root");
        let mut runner = IgnoreRunner::default();

        let error = config.validate(directory.path(), &mut runner).unwrap_err();

        assert!(error.to_string().contains("ignored by Git"));
    }

    #[test]
    fn exactly_one_file_source_is_required() {
        let mut config = valid_config();
        config.files.push(FileRule {
            label: "environment".to_owned(),
            destination: ".env".to_owned(),
            template: Some(".env.example".to_owned()),
            source_env: Some("LOCAL_ENV".to_owned()),
        });
        let directory = tempdir().expect("temporary directory");
        let mut runner = IgnoreRunner {
            ignored: true,
            ..Default::default()
        };

        let error = config.validate(directory.path(), &mut runner).unwrap_err();

        assert!(error.to_string().contains("exactly one"));
    }

    #[test]
    fn duplicate_file_destinations_are_rejected() {
        let mut config = valid_config();
        config.files = vec![
            FileRule {
                label: "first".to_owned(),
                destination: ".env".to_owned(),
                template: Some("first.env".to_owned()),
                source_env: None,
            },
            FileRule {
                label: "second".to_owned(),
                destination: ".env".to_owned(),
                template: Some("second.env".to_owned()),
                source_env: None,
            },
        ];
        let directory = tempdir().expect("temporary directory");
        let mut runner = IgnoreRunner {
            ignored: true,
            ..Default::default()
        };

        let error = config.validate(directory.path(), &mut runner).unwrap_err();

        assert!(error.to_string().contains("distinct files"));
    }

    #[test]
    fn dot_is_allowed_for_repository_relative_runtime_paths() {
        assert!(validate_repository_relative("app_root", ".").is_ok());
        assert!(validate_workspace_relative("check", ".").is_ok());
    }

    #[test]
    fn missing_and_malformed_configuration_report_the_named_file() {
        let directory = tempdir().expect("temporary directory");

        let missing = ProjectConfig::load(directory.path()).unwrap_err();
        fs::write(directory.path().join(CONFIG_FILE), "version = 'wrong'\n")
            .expect("malformed configuration");
        let malformed = ProjectConfig::load(directory.path()).unwrap_err();

        assert!(missing.to_string().contains(CONFIG_FILE));
        assert!(missing.to_string().contains("configuration is required"));
        assert!(malformed.to_string().contains(CONFIG_FILE));
        assert!(malformed.to_string().contains("invalid configuration"));
    }

    #[test]
    fn names_and_relative_paths_enforce_the_documented_grammar() {
        for invalid in [
            "",
            "Upper",
            "-leading",
            "trailing-",
            "two--dashes",
            "space here",
        ] {
            assert!(
                validate_name("workspace name", invalid).is_err(),
                "{invalid}"
            );
        }
        for invalid in ["", "/absolute", "../escape", "a/../../escape", "a/.."] {
            assert!(
                validate_repository_relative("path", invalid).is_err(),
                "{invalid}"
            );
        }
        assert!(validate_name("workspace name", "task-42").is_ok());
        assert!(validate_repository_relative("path", "a/../b").is_ok());
    }

    #[test]
    fn file_command_and_check_rules_reject_invalid_public_configuration() {
        let directory = tempdir().expect("temporary directory");
        fs::create_dir(directory.path().join(".worktrees")).expect("workspace root");
        let validate = |config: &ProjectConfig| {
            config
                .validate(
                    directory.path(),
                    &mut IgnoreRunner {
                        ignored: true,
                        ..Default::default()
                    },
                )
                .unwrap_err()
                .to_string()
        };

        let mut empty_file_label = valid_config();
        empty_file_label.files.push(FileRule {
            label: " ".to_owned(),
            destination: ".env".to_owned(),
            template: Some("example.env".to_owned()),
            source_env: None,
        });
        assert!(validate(&empty_file_label).contains("files[].label"));

        let mut missing_source = valid_config();
        missing_source.files.push(FileRule {
            label: "environment".to_owned(),
            destination: ".env".to_owned(),
            template: None,
            source_env: None,
        });
        assert!(validate(&missing_source).contains("exactly one"));

        let mut invalid_source_name = valid_config();
        invalid_source_name.files.push(FileRule {
            label: "environment".to_owned(),
            destination: ".env".to_owned(),
            template: None,
            source_env: Some("1INVALID".to_owned()),
        });
        assert!(validate(&invalid_source_name).contains("variable name"));

        let mut empty_command_label = valid_config();
        empty_command_label.commands.push(CommandRule {
            label: "".to_owned(),
            cwd: ".".to_owned(),
            argv: vec!["true".to_owned()],
            sensitive: false,
        });
        assert!(validate(&empty_command_label).contains("commands[].label"));

        let mut empty_argv = valid_config();
        empty_argv.commands.push(CommandRule {
            label: "setup".to_owned(),
            cwd: ".".to_owned(),
            argv: vec![String::new()],
            sensitive: false,
        });
        assert!(validate(&empty_argv).contains("non-empty argv"));

        for (kind, key, expected) in [
            ("path-exists", Some("KEY"), "cannot define"),
            ("env-key", None, "requires"),
            ("future-kind", None, "unsupported"),
        ] {
            let mut invalid_check = valid_config();
            invalid_check.checks.push(CheckRule {
                label: "health".to_owned(),
                kind: kind.to_owned(),
                path: ".env".to_owned(),
                key: key.map(str::to_owned),
            });
            assert!(validate(&invalid_check).contains(expected));
        }

        let mut valid = valid_config();
        valid.files.push(FileRule {
            label: "environment".to_owned(),
            destination: ".env".to_owned(),
            template: Some("example.env".to_owned()),
            source_env: None,
        });
        valid.commands.push(CommandRule {
            label: "setup".to_owned(),
            cwd: ".".to_owned(),
            argv: vec!["true".to_owned()],
            sensitive: false,
        });
        valid.checks.push(CheckRule {
            label: "environment".to_owned(),
            kind: "env-key".to_owned(),
            path: ".env".to_owned(),
            key: Some("APP_KEY".to_owned()),
        });
        assert!(
            valid
                .validate(
                    directory.path(),
                    &mut IgnoreRunner {
                        ignored: true,
                        ..Default::default()
                    }
                )
                .is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_paths_reject_symlink_escape_and_symlinked_ddev_metadata() {
        use std::os::unix::fs::symlink;

        let repository = tempdir().expect("repository");
        let outside = tempdir().expect("outside directory");
        symlink(outside.path(), repository.path().join("linked")).expect("escape symlink");

        let error = safe_join(repository.path(), "linked/file").unwrap_err();
        assert!(error.to_string().contains("through a symlink"));
        assert_eq!(
            nearest_existing_parent(&repository.path().join("missing/child")),
            repository.path()
        );

        fs::create_dir(repository.path().join(".worktrees")).expect("workspace root");
        fs::create_dir(repository.path().join("app")).expect("app root");
        symlink(outside.path(), repository.path().join("app/.ddev")).expect("DDEV symlink");
        let mut config = valid_config();
        config.ddev = Some(DdevConfig {
            app_root: "app".to_owned(),
            source_site: None,
        });
        let error = config
            .validate(
                repository.path(),
                &mut IgnoreRunner {
                    ignored: true,
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("symlinked .ddev"));
    }

    #[test]
    fn git_ignore_probe_is_repository_relative_and_outside_paths_are_rejected() {
        let repository = tempdir().expect("repository");
        let outside = tempdir().expect("outside directory");
        let mut runner = IgnoreRunner {
            ignored: true,
            ..Default::default()
        };

        assert!(
            is_ignored(
                repository.path(),
                &repository.path().join(".worktrees/probe"),
                &mut runner
            )
            .expect("ignore probe")
        );
        assert_eq!(runner.requests.borrow()[0].program, "git");
        assert!(is_ignored(repository.path(), outside.path(), &mut runner).is_err());
    }
}
