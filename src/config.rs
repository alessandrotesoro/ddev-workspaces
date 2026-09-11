use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::command::{CommandRequest, CommandRunner, ToolError, ToolResult};

pub const CONFIG_FILE: &str = ".ddev-workspaces.toml";
pub const CONFIG_VERSION: u32 = 2;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub version: u32,
    pub project_id: String,
    pub worktree_root: String,
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub environment: Option<EnvironmentConfig>,
    #[serde(default)]
    pub carry: Vec<CarryRule>,
    #[serde(default)]
    pub provision: Vec<ProvisionRule>,
    #[serde(default)]
    pub checks: Vec<CheckRule>,
    #[serde(default)]
    pub writable: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub adapter: String,
    pub host: String,
    #[serde(default)]
    pub docroot: Option<String>,
    #[serde(default)]
    pub component_kind: Option<String>,
    #[serde(default)]
    pub component_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    #[serde(default = "default_database_mode")]
    pub default: String,
    #[serde(default)]
    pub source: Option<String>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            default: default_database_mode(),
            source: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentConfig {
    pub source: String,
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub replace: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CarryRule {
    pub source: String,
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProvisionRule {
    #[serde(default)]
    pub label: Option<String>,
    pub argv: Vec<String>,
    #[serde(default = "default_command_cwd")]
    pub cwd: String,
    #[serde(default = "default_provision_moment")]
    pub when: String,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckRule {
    #[serde(default)]
    pub label: Option<String>,
    pub kind: String,
    pub path: String,
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub repository_root: PathBuf,
    pub host_root: PathBuf,
    pub worktree_root: PathBuf,
}

impl ProjectConfig {
    pub fn load(repo_root: &Path) -> ToolResult<Self> {
        let path = repo_root.join(CONFIG_FILE);
        let contents = fs::read_to_string(&path).map_err(|error| {
            ToolError::new(format!(
                "configuration is required at {}: {error}; add version = 2, project_id, worktree_root, and runtime",
                path.display()
            ))
        })?;
        toml::from_str::<Self>(&contents).map_err(|error| {
            ToolError::new(format!(
                "invalid configuration at {}: {error}; use the version 2 schema and rerun doctor",
                path.display()
            ))
        })
    }

    pub fn validate<R: CommandRunner>(
        &self,
        repo_root: &Path,
        runner: &mut R,
    ) -> ToolResult<RuntimePaths> {
        self.validate_structure(repo_root, runner)
    }

    pub fn validate_structure<R: CommandRunner>(
        &self,
        repo_root: &Path,
        runner: &mut R,
    ) -> ToolResult<RuntimePaths> {
        if self.version != CONFIG_VERSION {
            return Err(ToolError::new(format!(
                "configuration field `version` must be integer 2, found {}; only version 2 is accepted; update {}",
                self.version, CONFIG_FILE
            )));
        }
        let repository_root = fs::canonicalize(repo_root).map_err(|error| {
            ToolError::new(format!(
                "cannot resolve repository root {}: {error}",
                repo_root.display()
            ))
        })?;
        validate_name("project_id", &self.project_id)?;
        let worktree_root = resolve_worktree_root(&repository_root, &self.worktree_root)?;
        if worktree_root.starts_with(repository_root.join(".git")) {
            return Err(ToolError::new(
                "configuration field `worktree_root` must not be inside the Git metadata directory",
            ));
        }
        if worktree_root.starts_with(&repository_root)
            && !is_ignored(
                &repository_root,
                &worktree_root.join(".ddev-workspaces-ignore-probe"),
                runner,
            )?
        {
            return Err(ToolError::new(format!(
                "configuration field `worktree_root` must already be ignored by Git: {}; add a local ignore rule manually",
                worktree_root.display()
            )));
        }

        let host_root = resolve_host_root(&repository_root, &self.runtime.host)?;
        ensure_ddev_config(&host_root)?;
        if !repository_root.starts_with(&host_root) {
            return Err(ToolError::new(format!(
                "runtime host {} does not contain repository {}; the host must be the application containing this repository",
                host_root.display(),
                repository_root.display()
            )));
        }
        if !worktree_root.starts_with(&host_root) {
            return Err(ToolError::new(format!(
                "configured worktree root {} is outside runtime host {}; shared routing requires the host-mounted path",
                worktree_root.display(),
                host_root.display()
            )));
        }
        if worktree_root.starts_with(host_root.join(".git")) {
            return Err(ToolError::new(
                "configuration field `worktree_root` must not be inside the runtime host Git metadata directory",
            ));
        }

        match self.runtime.adapter.as_str() {
            "laravel" => {
                if repository_root != host_root {
                    return Err(ToolError::new(
                        "runtime adapter `laravel` requires runtime.host to be the Laravel repository root",
                    ));
                }
                let docroot = self.runtime.docroot.as_deref().unwrap_or(".");
                validate_workspace_relative("runtime.docroot", docroot)?;
                let _ = resolve_existing_or_future(&repository_root, docroot)?;
                if self.runtime.component_kind.is_some() || self.runtime.component_path.is_some() {
                    return Err(ToolError::new(
                        "runtime.adapter `laravel` cannot define component kind or component path",
                    ));
                }
            }
            "wordpress-component" => {
                if self.runtime.docroot.is_some() {
                    return Err(ToolError::new(
                        "runtime.adapter `wordpress-component` uses component_kind and component_path instead of docroot",
                    ));
                }
                let kind = self.runtime.component_kind.as_deref().ok_or_else(|| {
                    ToolError::new(
                        "runtime.adapter `wordpress-component` requires runtime.component_kind = `plugin` or `theme`",
                    )
                })?;
                if !matches!(kind, "plugin" | "theme") {
                    return Err(ToolError::new(
                        "runtime.component_kind must be exactly `plugin` or `theme`",
                    ));
                }
                let component_path = self.runtime.component_path.as_deref().ok_or_else(|| {
                    ToolError::new(
                        "runtime.adapter `wordpress-component` requires a parent-relative runtime.component_path",
                    )
                })?;
                validate_workspace_relative("runtime.component_path", component_path)?;
                let component = safe_join(&host_root, component_path)?;
                let canonical_component = fs::canonicalize(&component).map_err(|error| {
                    ToolError::new(format!(
                        "runtime.component_path {} is unavailable in host {}: {error}",
                        component_path,
                        host_root.display()
                    ))
                })?;
                if canonical_component != repository_root {
                    return Err(ToolError::new(format!(
                        "runtime.component_path resolves to {}, not this component repository {}",
                        canonical_component.display(),
                        repository_root.display()
                    )));
                }
                let expected_parent = if kind == "plugin" {
                    "plugins"
                } else {
                    "themes"
                };
                let normalized_component_path = normalize_relative_path(component_path);
                let components = normalized_component_path
                    .components()
                    .filter_map(|component| match component {
                        Component::Normal(part) => part.to_str(),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if components.len() < 3
                    || components[0] != "wp-content"
                    || components[1] != expected_parent
                    || components[components.len() - 2] != expected_parent
                {
                    return Err(ToolError::new(format!(
                        "runtime.component_path must be below wp-content/{expected_parent} for a {kind} workspace",
                    )));
                }
                if self.environment.is_some() || !self.carry.is_empty() || !self.writable.is_empty()
                {
                    return Err(ToolError::new(
                        "runtime.adapter `wordpress-component` does not accept environment, carry, or writable rules; use the generated wp-config and adapter-owned wp-content paths",
                    ));
                }
            }
            adapter => {
                return Err(ToolError::new(format!(
                    "runtime.adapter `{adapter}` is unsupported; use `laravel` or `wordpress-component`",
                )));
            }
        }

        validate_database(&self.database)?;
        validate_generated_destinations(self)?;
        validate_environment(self, &repository_root, runner)?;
        validate_carry(self, &repository_root, runner)?;
        validate_provision(self)?;
        validate_checks(self)?;
        validate_writable(self)?;

        Ok(RuntimePaths {
            repository_root,
            host_root,
            worktree_root,
        })
    }

    pub fn database_mode(&self) -> ToolResult<DatabaseMode> {
        match self.database.default.as_str() {
            "copy" => Ok(DatabaseMode::Copy),
            "fresh" => Ok(DatabaseMode::Fresh),
            value => Err(ToolError::new(format!(
                "database.default `{value}` is unsupported; use `copy` or `fresh`",
            ))),
        }
    }

    pub fn environment_source(
        &self,
        paths: &RuntimePaths,
    ) -> ToolResult<Option<(PathBuf, PathBuf)>> {
        let Some(environment) = &self.environment else {
            return Ok(None);
        };
        let source = resolve_configured_source(&paths.repository_root, &environment.source)?;
        let destination = environment
            .destination
            .as_deref()
            .unwrap_or(&environment.source);
        validate_workspace_relative("environment.destination", destination)?;
        reject_directory_destination("environment.destination", destination)?;
        Ok(Some((source, normalize_relative_path(destination))))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseMode {
    Copy,
    Fresh,
}

fn default_database_mode() -> String {
    "copy".to_owned()
}

fn default_command_cwd() -> String {
    ".".to_owned()
}

fn default_provision_moment() -> String {
    "always".to_owned()
}

fn validate_database(database: &DatabaseConfig) -> ToolResult<()> {
    if !matches!(database.default.as_str(), "copy" | "fresh") {
        return Err(ToolError::new(format!(
            "database.default `{}` is unsupported; use `copy` or `fresh`",
            database.default
        )));
    }
    if let Some(source) = &database.source {
        validate_database_name("database.source", source)?;
    }
    Ok(())
}

fn validate_generated_destinations(config: &ProjectConfig) -> ToolResult<()> {
    if config.runtime.adapter != "laravel" {
        return Ok(());
    }
    let docroot = normalize_relative_path(config.runtime.docroot.as_deref().unwrap_or("."));
    let is_public = |destination: &str| {
        let destination = normalize_relative_path(destination);
        destination == docroot || destination.starts_with(&docroot)
    };
    if let Some(environment) = &config.environment {
        let destination = environment
            .destination
            .as_deref()
            .unwrap_or(&environment.source);
        if is_public(destination) {
            return Err(ToolError::new(format!(
                "generated environment destination `{destination}` would be inside Laravel runtime.docroot; keep generated environment state outside the public docroot"
            )));
        }
    }
    for carry in &config.carry {
        if !carry.sensitive {
            continue;
        }
        let destination = carry.destination.as_deref().unwrap_or(&carry.source);
        if is_public(destination) {
            return Err(ToolError::new(format!(
                "sensitive carry destination `{destination}` would be inside Laravel runtime.docroot; keep sensitive generated state outside the public docroot"
            )));
        }
    }
    Ok(())
}

fn validate_environment<R: CommandRunner>(
    config: &ProjectConfig,
    repository_root: &Path,
    runner: &mut R,
) -> ToolResult<()> {
    let Some(environment) = &config.environment else {
        return Ok(());
    };
    validate_repository_relative("environment.source", &environment.source)?;
    let source = resolve_configured_source(repository_root, &environment.source)?;
    ensure_regular_file(&source, "environment source")?;
    let destination = environment
        .destination
        .as_deref()
        .unwrap_or(&environment.source);
    validate_workspace_relative("environment.destination", destination)?;
    reject_directory_destination("environment.destination", destination)?;
    let destination = normalize_relative_path(destination);
    if !is_ignored(repository_root, &repository_root.join(&destination), runner)? {
        return Err(ToolError::new(format!(
            "environment destination `{}` must already be ignored by Git because it contains generated workspace state",
            destination.display()
        )));
    }
    let mut replacements = Vec::new();
    for key in &environment.replace {
        validate_environment_name(key)?;
        if !matches!(
            key.as_str(),
            "APP_URL" | "DB_DATABASE" | "DB_HOST" | "DB_USERNAME" | "DB_PASSWORD"
        ) {
            return Err(ToolError::new(format!(
                "environment.replace key `{key}` is not workspace-owned; use APP_URL or the supported DB_* keys",
            )));
        }
        if replacements
            .iter()
            .any(|existing: &&String| existing.as_str() == key.as_str())
        {
            return Err(ToolError::new(format!(
                "environment.replace contains duplicate key `{key}`",
            )));
        }
        replacements.push(key);
    }
    if config.runtime.adapter == "laravel"
        && !replacements.iter().any(|key| key.as_str() == "DB_DATABASE")
    {
        return Err(ToolError::new(
            "Laravel environment.replace must include DB_DATABASE so a workspace cannot use the source database",
        ));
    }
    Ok(())
}

fn validate_carry<R: CommandRunner>(
    config: &ProjectConfig,
    repository_root: &Path,
    runner: &mut R,
) -> ToolResult<()> {
    let mut destinations = Vec::new();
    for carry in &config.carry {
        validate_repository_relative("carry.source", &carry.source)?;
        let source = resolve_configured_source(repository_root, &carry.source)?;
        if carry.required {
            ensure_regular_file(&source, "required carry source")?;
        } else if source.exists() {
            ensure_regular_file(&source, "carry source")?;
        }
        let destination = carry.destination.as_deref().unwrap_or(&carry.source);
        validate_workspace_relative("carry.destination", destination)?;
        let destination = normalize_relative_path(destination);
        reject_directory_destination("carry.destination", &destination.display().to_string())?;
        if carry.sensitive
            && !is_ignored(repository_root, &repository_root.join(&destination), runner)?
        {
            return Err(ToolError::new(format!(
                "sensitive carry destination `{}` must already be ignored by Git",
                destination.display()
            )));
        }
        if destinations.iter().any(|existing: &PathBuf| {
            existing == &destination
                || existing.starts_with(&destination)
                || destination.starts_with(existing)
        }) {
            return Err(ToolError::new(format!(
                "carry destination `{}` overlaps another carried path",
                destination.display()
            )));
        }
        destinations.push(destination);
    }
    Ok(())
}

fn validate_provision(config: &ProjectConfig) -> ToolResult<()> {
    for provision in &config.provision {
        if provision
            .label
            .as_deref()
            .is_some_and(|label| label.trim().is_empty())
        {
            return Err(ToolError::new(
                "provision.label must not be empty when provided",
            ));
        }
        if provision.argv.is_empty() || provision.argv.iter().any(String::is_empty) {
            return Err(ToolError::new(
                "each provision rule must have a non-empty argv array",
            ));
        }
        validate_repository_relative("provision.cwd", &provision.cwd)?;
        if !matches!(provision.when.as_str(), "always" | "copy" | "fresh") {
            return Err(ToolError::new(format!(
                "provision rule `{}` has unsupported when `{}`; use `always`, `copy`, or `fresh`",
                provision_label(provision),
                provision.when
            )));
        }
    }
    Ok(())
}

fn validate_checks(config: &ProjectConfig) -> ToolResult<()> {
    for check in &config.checks {
        if check
            .label
            .as_deref()
            .is_some_and(|label| label.trim().is_empty())
        {
            return Err(ToolError::new(
                "check.label must not be empty when provided",
            ));
        }
        validate_workspace_relative("check.path", &check.path)?;
        match check.kind.as_str() {
            "path-exists" if check.key.is_none() => {}
            "env-key" if check.key.as_deref().is_some_and(|key| !key.is_empty()) => {
                validate_environment_name(check.key.as_deref().unwrap_or_default())?;
            }
            "path-exists" => {
                return Err(ToolError::new(format!(
                    "check `{}` of kind `path-exists` cannot define key",
                    check_label(check)
                )));
            }
            "env-key" => {
                return Err(ToolError::new(format!(
                    "check `{}` of kind `env-key` requires a non-empty key",
                    check_label(check)
                )));
            }
            kind => {
                return Err(ToolError::new(format!(
                    "check `{}` has unsupported kind `{kind}`; use `path-exists` or `env-key`",
                    check_label(check)
                )));
            }
        }
    }
    Ok(())
}

fn validate_writable(config: &ProjectConfig) -> ToolResult<()> {
    let mut paths = Vec::new();
    for path in &config.writable {
        validate_workspace_relative("writable", path)?;
        reject_directory_destination("writable", path)?;
        let normalized = normalize_relative_path(path);
        if paths.iter().any(|existing: &PathBuf| {
            existing == &normalized
                || existing.starts_with(&normalized)
                || normalized.starts_with(existing)
        }) {
            return Err(ToolError::new(format!(
                "writable path `{}` overlaps another writable path",
                normalized.display()
            )));
        }
        paths.push(normalized);
    }
    Ok(())
}

fn reject_directory_destination(field: &str, value: &str) -> ToolResult<()> {
    if normalize_relative_path(value).as_os_str().is_empty() {
        return Err(ToolError::new(format!(
            "configuration field `{field}` must name a file or directory below the workspace root",
        )));
    }
    Ok(())
}

fn provision_label(rule: &ProvisionRule) -> String {
    rule.label.clone().unwrap_or_else(|| rule.argv.join(" "))
}

fn check_label(check: &CheckRule) -> String {
    check.label.clone().unwrap_or_else(|| check.path.clone())
}

pub fn resolve_host_root(repo_root: &Path, value: &str) -> ToolResult<PathBuf> {
    let path = Path::new(value);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        ToolError::new(format!(
            "runtime.host {} is unavailable: {error}",
            candidate.display()
        ))
    })?;
    if !canonical.is_dir() || canonical.join(".ddev").is_symlink() {
        return Err(ToolError::new(format!(
            "runtime.host {} must be a regular directory with a non-symlink .ddev directory",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn ensure_ddev_config(host_root: &Path) -> ToolResult<()> {
    let directory = host_root.join(".ddev");
    let metadata = fs::symlink_metadata(&directory).map_err(|error| {
        ToolError::new(format!(
            "runtime host {} has no usable .ddev directory: {error}",
            host_root.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ToolError::new(format!(
            "runtime host {} must contain a regular .ddev directory",
            host_root.display()
        )));
    }
    let config = directory.join("config.yaml");
    ensure_regular_file(&config, "DDEV configuration")
}

fn resolve_worktree_root(repo_root: &Path, value: &str) -> ToolResult<PathBuf> {
    if value.is_empty() || Path::new(value).is_absolute() {
        return Err(ToolError::new(
            "configuration field `worktree_root` must be a non-empty relative path",
        ));
    }
    let candidate = normalize_absolute_path(&repo_root.join(value));
    let parent = nearest_existing_parent(&candidate);
    let canonical_parent = fs::canonicalize(&parent).map_err(|error| {
        ToolError::new(format!(
            "cannot resolve worktree root parent {}: {error}",
            parent.display()
        ))
    })?;
    if parent != canonical_parent {
        return Err(ToolError::new(
            "configuration field `worktree_root` resolves through a symlink; use a canonical path",
        ));
    }
    let relative = candidate
        .strip_prefix(&parent)
        .map_err(|_| ToolError::new("cannot resolve worktree root"))?;
    Ok(canonical_parent.join(normalize_relative_path(&relative.display().to_string())))
}

fn resolve_existing_or_future(root: &Path, relative: &str) -> ToolResult<PathBuf> {
    let candidate = safe_join(root, relative)?;
    let parent = nearest_existing_parent(&candidate);
    let canonical_parent = fs::canonicalize(&parent).map_err(|error| {
        ToolError::new(format!(
            "cannot resolve path parent {}: {error}",
            parent.display()
        ))
    })?;
    let canonical_root = fs::canonicalize(root)?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(ToolError::new(format!(
            "path `{relative}` escapes its root through a symlink"
        )));
    }
    Ok(candidate)
}

fn resolve_configured_source(root: &Path, value: &str) -> ToolResult<PathBuf> {
    if Path::new(value).is_absolute() {
        return Err(ToolError::new(format!(
            "configured source path `{value}` must be repository-relative",
        )));
    }
    safe_join(root, value)
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

pub fn validate_workspace_name(value: &str) -> ToolResult<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.starts_with('-')
        || value.ends_with('/')
        || value.starts_with('/')
        || value.contains("//")
        || value.contains("..")
        || value.contains("@{")
        || value.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || !matches!(
                    byte,
                    b'a'..=b'z'
                        | b'A'..=b'Z'
                        | b'0'..=b'9'
                        | b'-'
                        | b'_'
                        | b'.'
                        | b'/'
                )
        })
        || value.split('/').any(|component| {
            component.is_empty()
                || component.starts_with('.')
                || component.ends_with('.')
                || component.ends_with(".lock")
                || component
                    .bytes()
                    .any(|byte| matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\'))
        })
    {
        return Err(ToolError::new(format!(
            "workspace name `{value}` is not a safe Git branch name; use letters, digits, `/`, `.`, `_`, and `-`"
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
                    "configuration field `{field}` escapes its root"
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
        ToolError::new(format!("cannot resolve root {}: {error}", root.display()))
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
            "path `{relative}` escapes root through a symlink"
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

fn normalize_absolute_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                normalized.pop();
            }
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
    match output.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(ToolError::new(format!(
            "Git could not inspect whether {} is ignored",
            path.display()
        ))),
    }
}

pub fn validate_database_name(field: &str, value: &str) -> ToolResult<()> {
    if value.is_empty()
        || value.len() > 60
        || value
            .bytes()
            .any(|byte| !matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_'))
    {
        return Err(ToolError::new(format!(
            "{field} `{value}` must be 1-60 lowercase database-safe characters"
        )));
    }
    Ok(())
}

pub fn resource_key(project_id: &str, workspace_name: &str) -> String {
    let project = normalize_identity_part(project_id);
    let workspace = normalize_identity_part(workspace_name);
    let raw = format!("{project}-{workspace}");
    if raw.len() <= 52 {
        return raw;
    }
    let suffix = stable_hash(&format!("{project_id}\0{workspace_name}"));
    format!("{}-{suffix}", &raw[..52 - suffix.len() - 1])
}

pub fn identity_hash(project_id: &str, workspace_name: &str) -> String {
    stable_hash(&format!("{project_id}\0{workspace_name}"))
}

fn normalize_identity_part(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('-');
            }
            output.push(byte.to_ascii_lowercase() as char);
            separator = false;
        } else if !output.is_empty() {
            separator = true;
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "workspace".to_owned()
    } else {
        output
    }
}

fn stable_hash(value: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:08x}")
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

fn ensure_regular_file(path: &Path, label: &str) -> ToolResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ToolError::new(format!(
            "{label} {} is unavailable: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(ToolError::new(format!(
            "{label} {} must be a regular non-symlink file",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
