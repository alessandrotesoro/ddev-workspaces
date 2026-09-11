use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::command::{CommandRequest, CommandRunner, ToolError, ToolResult};
use crate::state;

const CONFIG_ASSET: &str = include_str!("../assets/ddev/config.ddev-workspaces.yaml.tmpl");
const ENTRYPOINT_ASSET: &str = include_str!("../assets/ddev/web-entrypoint.d/ddev-workspaces.sh");
const ASSET_MARKER: &str = "# ddev-workspaces: managed asset; do not edit";

struct TemporaryAssetGuard {
    path: PathBuf,
}

impl Drop for TemporaryAssetGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DdevProject {
    pub name: String,
    pub approot: String,
    pub status: String,
    pub hostname: String,
    pub hostnames: Vec<String>,
    pub httpsurl: String,
    pub project_tld: String,
    #[serde(rename = "type")]
    pub project_type: String,
    pub webserver_type: String,
    pub database_type: String,
    pub docroot: String,
    pub mutagen_enabled: bool,
    pub mutagen_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostIdentity {
    pub project: DdevProject,
    pub hostname: String,
}

impl HostIdentity {
    pub fn same_runtime_identity(&self, other: &Self) -> bool {
        self.hostname == other.hostname
            && self.project.name == other.project.name
            && self.project.approot == other.project.approot
            && self.project.hostname == other.project.hostname
            && self.project.hostnames == other.project.hostnames
            && self.project.httpsurl == other.project.httpsurl
            && self.project.project_tld == other.project.project_tld
            && self.project.project_type == other.project.project_type
            && self.project.webserver_type == other.project.webserver_type
            && self.project.database_type == other.project.database_type
            && self.project.docroot == other.project.docroot
            && self.project.mutagen_enabled == other.project.mutagen_enabled
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdevInspection {
    pub entries: Vec<DdevProject>,
    pub warnings: Vec<String>,
}

pub fn list<R: CommandRunner>(runner: &mut R, cwd: &Path) -> ToolResult<DdevInspection> {
    let shadow = prepare_list_shadow(runner, cwd)?;
    let request = CommandRequest::new("ddev", ["list", "--json-output"])
        .cwd(cwd)
        .env(
            "DDEV_XDG_CONFIG_HOME",
            shadow.xdg_home.to_string_lossy().into_owned(),
        );
    let output = runner.run(&request)?;
    if !output.success() {
        return Err(ToolError::new(
            "DDEV list failed; verify that DDEV is installed and retry the read-only inspection",
        ));
    }
    let entries: Vec<DdevProject> = parse_json_output(&output.stdout, "DDEV list")?;
    let mut warnings = parse_warnings(&output.stderr);
    for entry in &entries {
        if !Path::new(&entry.approot).exists() {
            warnings.push(format!(
                "stale DDEV registration '{}' points to missing path {}",
                entry.name, entry.approot
            ));
        }
    }
    for left in 0..entries.len() {
        for right in left + 1..entries.len() {
            if entries[left].name == entries[right].name {
                warnings.push(format!(
                    "duplicate DDEV name '{}' is registered more than once",
                    entries[left].name
                ));
            }
            if canonical_or_raw(Path::new(&entries[left].approot))
                == canonical_or_raw(Path::new(&entries[right].approot))
            {
                warnings.push(format!(
                    "duplicate DDEV app root '{}' is registered under multiple names",
                    entries[left].approot
                ));
            }
        }
    }
    Ok(DdevInspection { entries, warnings })
}

pub fn inspect_host<R: CommandRunner>(
    host_root: &Path,
    runner: &mut R,
) -> ToolResult<HostIdentity> {
    let inspection = list(runner, host_root)?;
    let expected_root = canonical_or_raw(host_root);
    let matches = inspection
        .entries
        .iter()
        .filter(|entry| canonical_or_raw(Path::new(&entry.approot)) == expected_root)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(ToolError::new(format!(
            "DDEV host {} is not represented by exactly one registered project; run 'ddev list' manually and repair the host identity",
            host_root.display()
        )));
    }
    let mut project = matches[0].clone();
    if let Ok(described) = describe(host_root, runner) {
        merge_project_details(&mut project, described);
    }
    if !project.status.eq_ignore_ascii_case("running") {
        return Err(ToolError::new(format!(
            "DDEV host '{}' is not running (status '{}'); start it manually and rerun doctor",
            project.name, project.status
        )));
    }
    if !project.webserver_type.eq_ignore_ascii_case("nginx-fpm") {
        return Err(ToolError::new(format!(
            "DDEV host '{}' uses webserver '{}'; shared routing requires nginx-fpm",
            project.name, project.webserver_type
        )));
    }
    if !matches!(
        project.database_type.to_ascii_lowercase().as_str(),
        "mysql" | "mariadb"
    ) {
        return Err(ToolError::new(format!(
            "DDEV host '{}' uses database '{}'; shared workspaces support MySQL or MariaDB only",
            project.name, project.database_type
        )));
    }
    let hostname = if !project.hostname.trim().is_empty() {
        project.hostname.clone()
    } else if !project.httpsurl.trim().is_empty() {
        project
            .httpsurl
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_owned()
    } else {
        project
            .hostnames
            .first()
            .cloned()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ToolError::new("DDEV host did not report a usable hostname"))?
    };
    validate_host_hostname(&hostname)?;
    Ok(HostIdentity { project, hostname })
}

fn describe<R: CommandRunner>(host_root: &Path, runner: &mut R) -> ToolResult<DdevProject> {
    let output =
        runner.run(&CommandRequest::new("ddev", ["describe", "--json-output"]).cwd(host_root))?;
    if !output.success() {
        return Err(ToolError::new(
            "DDEV describe failed; inspect the registered host manually",
        ));
    }
    parse_json_output(&output.stdout, "DDEV describe")
}

fn merge_project_details(base: &mut DdevProject, details: DdevProject) {
    if !details.name.is_empty() {
        base.name = details.name;
    }
    if !details.approot.is_empty() {
        base.approot = details.approot;
    }
    if !details.status.is_empty() {
        base.status = details.status;
    }
    if !details.hostname.is_empty() {
        base.hostname = details.hostname;
    }
    if !details.hostnames.is_empty() {
        base.hostnames = details.hostnames;
    }
    if !details.httpsurl.is_empty() {
        base.httpsurl = details.httpsurl;
    }
    if !details.project_tld.is_empty() {
        base.project_tld = details.project_tld;
    }
    if !details.project_type.is_empty() {
        base.project_type = details.project_type;
    }
    if !details.webserver_type.is_empty() {
        base.webserver_type = details.webserver_type;
    }
    if !details.database_type.is_empty() {
        base.database_type = details.database_type;
    }
    if !details.docroot.is_empty() {
        base.docroot = details.docroot;
    }
    base.mutagen_enabled |= details.mutagen_enabled;
    if !details.mutagen_status.is_empty() {
        base.mutagen_status = details.mutagen_status;
    }
}

pub fn require_initialized(host_root: &Path, identity: &HostIdentity) -> ToolResult<()> {
    let directory = host_root.join(".ddev");
    let config_path = directory.join("config.ddev-workspaces.yaml");
    let entrypoint_path = directory.join("web-entrypoint.d/ddev-workspaces.sh");
    let config = fs::read_to_string(&config_path).map_err(|error| {
        ToolError::new(format!(
            "shared host is not initialized: cannot read {}; run 'ddev-workspaces host init': {error}",
            config_path.display()
        ))
    })?;
    let entrypoint = fs::read_to_string(&entrypoint_path).map_err(|error| {
        ToolError::new(format!(
            "shared host is not initialized: cannot read {}; run 'ddev-workspaces host init': {error}",
            entrypoint_path.display()
        ))
    })?;
    let expected_config = CONFIG_ASSET.replace("{{HOST_NAME}}", &identity.project.name);
    if config != expected_config || entrypoint != ENTRYPOINT_ASSET {
        return Err(ToolError::new(
            "shared host routing assets are stale, foreign, or incomplete for the current DDEV identity; run 'ddev-workspaces host init' after reviewing the files",
        ));
    }
    if !entrypoint_is_executable(&entrypoint_path)? {
        return Err(ToolError::new(
            "shared host routing entrypoint is not executable; run 'ddev-workspaces host init' before creating or removing workspaces",
        ));
    }
    if state::restart_required(host_root)? {
        return Err(ToolError::new(
            "shared host routing restart is pending; run 'ddev-workspaces host init' and wait for it to succeed before creating or removing workspaces",
        ));
    }
    Ok(())
}

pub fn init_host<R: CommandRunner>(
    host_root: &Path,
    identity: &HostIdentity,
    runner: &mut R,
) -> ToolResult<bool> {
    state::ensure_state(host_root)?;
    let directory = host_root.join(".ddev");
    ensure_regular_directory(&directory, ".ddev directory")?;
    let entrypoint_directory = directory.join("web-entrypoint.d");
    ensure_directory(&entrypoint_directory, "DDEV web-entrypoint directory")?;
    let config_contents = CONFIG_ASSET.replace("{{HOST_NAME}}", &identity.project.name);
    let config_changed = asset_needs_update(
        &directory.join("config.ddev-workspaces.yaml"),
        config_contents.as_bytes(),
    )?;
    let entrypoint_path = entrypoint_directory.join("ddev-workspaces.sh");
    let entrypoint_changed = asset_needs_update(&entrypoint_path, ENTRYPOINT_ASSET.as_bytes())?;
    let entrypoint_mode_changed = !entrypoint_is_executable(&entrypoint_path)?;
    if config_changed || entrypoint_changed || entrypoint_mode_changed {
        state::mark_restart_required(host_root)?;
    }
    let config_changed = write_owned_asset(
        &directory.join("config.ddev-workspaces.yaml"),
        config_contents.as_bytes(),
    )?;
    let entrypoint_changed = write_owned_asset(&entrypoint_path, ENTRYPOINT_ASSET.as_bytes())?;
    set_executable(&entrypoint_path)?;
    let restart_required = state::restart_required(host_root)?;
    if config_changed || entrypoint_changed || entrypoint_mode_changed || restart_required {
        let output = runner.run(
            &CommandRequest::new("ddev", ["restart", "-y"])
                .cwd(host_root)
                .mutating(),
        )?;
        if !output.success() {
            return Err(ToolError::new(
                "DDEV host restart failed while installing shared routing; inspect DDEV manually before retrying",
            ));
        }
        state::clear_restart_required(host_root)?;
    }
    Ok(config_changed || entrypoint_changed || entrypoint_mode_changed || restart_required)
}

pub fn expected_hostname(hostname: &str, resource_key: &str) -> ToolResult<String> {
    validate_hostname_part(resource_key)?;
    validate_host_hostname(hostname)?;
    let value = format!("{resource_key}.{hostname}");
    if value.len() > 253 {
        return Err(ToolError::new(
            "workspace hostname exceeds the DNS name limit",
        ));
    }
    Ok(value)
}

fn validate_host_hostname(hostname: &str) -> ToolResult<()> {
    if hostname.is_empty() || hostname.len() > 253 {
        return Err(ToolError::new(format!(
            "DDEV host hostname '{}' is not usable for workspace routing",
            hostname
        )));
    }
    for label in hostname.split('.') {
        if label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || !label.as_bytes()[0].is_ascii_alphanumeric()
            || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
        {
            return Err(ToolError::new(format!(
                "DDEV host hostname '{}' is not usable for workspace routing",
                hostname
            )));
        }
    }
    Ok(())
}

pub fn expected_url(hostname: &str, resource_key: &str) -> ToolResult<String> {
    Ok(format!(
        "https://{}",
        expected_hostname(hostname, resource_key)?
    ))
}

pub fn publish_link(host_root: &Path, resource_key: &str, target: &Path) -> ToolResult<PathBuf> {
    validate_hostname_part(resource_key)?;
    let link = state::links_directory(host_root).join(resource_key);
    let target_metadata = fs::symlink_metadata(target).map_err(|error| {
        ToolError::new(format!(
            "workspace route target {} cannot be inspected: {error}",
            target.display()
        ))
    })?;
    if target_metadata.file_type().is_symlink() || !target_metadata.is_dir() {
        return Err(ToolError::new(format!(
            "workspace route target {} is not a regular directory",
            target.display()
        )));
    }
    let canonical_host_root = fs::canonicalize(host_root)?;
    let expected = fs::canonicalize(target)?;
    if !expected.starts_with(&canonical_host_root) {
        return Err(ToolError::new(format!(
            "workspace route target {} escapes the shared host root {}",
            target.display(),
            canonical_host_root.display()
        )));
    }
    if let Some(parent) = link.parent() {
        ensure_regular_directory(parent, "workspace link directory")?;
        let canonical_parent = fs::canonicalize(parent)?;
        if !canonical_parent.starts_with(&canonical_host_root) {
            return Err(ToolError::new(format!(
                "workspace route parent {} escapes the shared host root {}",
                parent.display(),
                canonical_host_root.display()
            )));
        }
    } else {
        return Err(ToolError::new("workspace route link has no parent"));
    }
    match fs::symlink_metadata(&link) {
        Ok(metadata) => {
            if !metadata.file_type().is_symlink() {
                return Err(ToolError::new(format!(
                    "workspace route {} already exists and is not an owned symlink",
                    link.display()
                )));
            }
            let actual = fs::canonicalize(resolve_link_target(&link)?)?;
            if actual != expected {
                return Err(ToolError::new(format!(
                    "workspace route {} points to {}, not {}",
                    link.display(),
                    actual.display(),
                    expected.display()
                )));
            }
            return Ok(link);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ToolError::new(format!(
                "cannot inspect workspace route link {}: {error}",
                link.display()
            )));
        }
    }
    let canonical_parent = fs::canonicalize(
        link.parent()
            .ok_or_else(|| ToolError::new("workspace route link has no parent"))?,
    )?;
    let current_parent = fs::canonicalize(
        link.parent()
            .ok_or_else(|| ToolError::new("workspace route link has no parent"))?,
    )?;
    if current_parent != canonical_parent || !current_parent.starts_with(&canonical_host_root) {
        return Err(ToolError::new(
            "workspace route parent changed while publishing; retry the operation",
        ));
    }
    let relative_target = relative_path_from_canonical(&canonical_parent, &expected)?;
    create_symlink(&relative_target, &link)?;
    Ok(link)
}

pub fn unpublish_link(host_root: &Path, resource_key: &str, target: &Path) -> ToolResult<()> {
    validate_hostname_part(resource_key)?;
    let link = state::links_directory(host_root).join(resource_key);
    let metadata = match fs::symlink_metadata(&link) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ToolError::new(format!(
                "cannot inspect workspace route link {}: {error}",
                link.display()
            )));
        }
    };
    if !metadata.file_type().is_symlink() {
        return Err(ToolError::new(format!(
            "workspace route {} is not an owned symlink; refusing removal",
            link.display()
        )));
    }
    let canonical_host_root = fs::canonicalize(host_root)?;
    let parent = link
        .parent()
        .ok_or_else(|| ToolError::new("workspace route link has no parent"))?;
    ensure_regular_directory(parent, "workspace link directory")?;
    let canonical_parent = fs::canonicalize(parent)?;
    if !canonical_parent.starts_with(&canonical_host_root) {
        return Err(ToolError::new(format!(
            "workspace route parent {} escapes the shared host root {}",
            parent.display(),
            canonical_host_root.display()
        )));
    }
    let expected_canonical = canonicalize_route_target(target)?;
    let actual = resolve_link_target(&link)?;
    let matches = canonicalize_route_target(&actual)? == expected_canonical;
    if !matches {
        return Err(ToolError::new(format!(
            "workspace route {} points to an unexpected target instead of the recorded target {}; refusing removal",
            link.display(),
            target.display()
        )));
    }
    fs::remove_file(&link)?;
    Ok(())
}

pub fn link_target(link: &Path) -> ToolResult<PathBuf> {
    fs::read_link(link).map_err(|error| {
        ToolError::new(format!(
            "cannot read workspace route {}: {error}",
            link.display()
        ))
    })
}

fn resolve_link_target(link: &Path) -> ToolResult<PathBuf> {
    let target = link_target(link)?;
    if target.is_absolute() {
        Ok(target)
    } else {
        link.parent()
            .map(|parent| parent.join(target))
            .ok_or_else(|| ToolError::new("workspace route link has no parent"))
    }
}

pub(crate) fn relative_path(from_directory: &Path, target: &Path) -> ToolResult<PathBuf> {
    let from = fs::canonicalize(from_directory)?;
    let target = fs::canonicalize(target)?;
    relative_path_from_canonical(&from, &target)
}

fn relative_path_from_canonical(from: &Path, target: &Path) -> ToolResult<PathBuf> {
    let from_components = from.components().collect::<Vec<_>>();
    let target_components = target.components().collect::<Vec<_>>();
    let common = from_components
        .iter()
        .zip(&target_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return Err(ToolError::new(
            "workspace route target and link are on incompatible filesystem roots",
        ));
    }
    let mut relative = PathBuf::new();
    for _ in common..from_components.len() {
        relative.push("..");
    }
    for component in &target_components[common..] {
        relative.push(component.as_os_str());
    }
    if relative.as_os_str().is_empty() {
        relative.push(".");
    }
    Ok(relative)
}

fn canonicalize_route_target(path: &Path) -> ToolResult<PathBuf> {
    match fs::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut missing = Vec::new();
            let mut existing = path.to_path_buf();
            loop {
                match fs::symlink_metadata(&existing) {
                    Ok(_) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        let name = existing.file_name().ok_or_else(|| {
                            ToolError::new(format!(
                                "cannot resolve missing route target {}",
                                path.display()
                            ))
                        })?;
                        missing.push(name.to_owned());
                        existing = existing
                            .parent()
                            .ok_or_else(|| {
                                ToolError::new(format!(
                                    "cannot resolve missing route target {}",
                                    path.display()
                                ))
                            })?
                            .to_path_buf();
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let mut canonical = fs::canonicalize(existing)?;
            for component in missing.iter().rev() {
                canonical.push(component);
            }
            Ok(canonical)
        }
        Err(error) => Err(error.into()),
    }
}

pub fn sync_mutagen<R: CommandRunner>(
    host_root: &Path,
    resource_key: &str,
    runner: &mut R,
) -> ToolResult<()> {
    let sync = runner.run(
        &CommandRequest::new("ddev", ["mutagen", "sync"])
            .cwd(host_root)
            .mutating(),
    )?;
    if !sync.success() {
        return Err(ToolError::new(
            "DDEV Mutagen synchronization failed; repair the shared host manually",
        ));
    }
    let path = format!(
        "/var/www/html/{}/{}/{resource_key}",
        state::STATE_DIRECTORY,
        state::LINKS_DIRECTORY
    );
    let probe = runner.run(
        &CommandRequest::new(
            "ddev",
            [
                "exec",
                "--raw",
                "--service",
                "web",
                "--",
                "test",
                "-e",
                &path,
            ],
        )
        .cwd(host_root),
    )?;
    if !probe.success() {
        return Err(ToolError::new(
            "the shared route directory is not visible inside the DDEV web container; repair Mutagen or the host mount manually",
        ));
    }
    Ok(())
}

fn prepare_list_shadow<R: CommandRunner>(runner: &mut R, cwd: &Path) -> ToolResult<DdevListShadow> {
    let output = runner.run(&CommandRequest::new("ddev", ["version", "--json-output"]).cwd(cwd))?;
    if !output.success() {
        return Err(ToolError::new(
            "DDEV version inspection failed; verify that DDEV is installed",
        ));
    }
    let version: DdevVersion = parse_json_output(&output.stdout, "DDEV version")?;
    if !version.global_dir.is_absolute() {
        return Err(ToolError::new(
            "DDEV reported a non-absolute global configuration directory; refusing inspection",
        ));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ToolError::new(format!("system clock error: {error}")))?
        .as_nanos();
    let xdg_home = std::env::temp_dir().join(format!(
        "ddev-workspaces-list-{}-{nonce}",
        std::process::id()
    ));
    let shadow_directory = xdg_home.join("ddev");
    fs::create_dir_all(&shadow_directory)?;
    for filename in ["global_config.yaml", "project_list.yaml"] {
        let source = version.global_dir.join(filename);
        if source.is_file() {
            fs::copy(&source, shadow_directory.join(filename)).map_err(|error| {
                ToolError::new(format!(
                    "could not isolate DDEV {filename} for read-only inspection: {error}"
                ))
            })?;
        }
    }
    #[cfg(unix)]
    {
        let source = version.global_dir.join("bin");
        if source.is_dir() {
            std::os::unix::fs::symlink(source, shadow_directory.join("bin")).map_err(|error| {
                ToolError::new(format!(
                    "could not expose DDEV helper binaries to isolated inspection: {error}"
                ))
            })?;
        }
    }
    Ok(DdevListShadow { xdg_home })
}

struct DdevListShadow {
    xdg_home: PathBuf,
}

impl Drop for DdevListShadow {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.xdg_home);
    }
}

#[derive(Debug, Deserialize)]
struct DdevVersion {
    #[serde(rename = "global-ddev-dir")]
    global_dir: PathBuf,
}

fn parse_json_output<T: DeserializeOwned>(output: &str, label: &str) -> ToolResult<T> {
    for line in output.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            let raw = value.get("raw").cloned().unwrap_or(value);
            if let Ok(parsed) = serde_json::from_value::<T>(raw) {
                return Ok(parsed);
            }
        }
    }
    serde_json::from_str::<T>(output).map_err(|error| {
        ToolError::new(format!(
            "{label} returned unsupported JSON output: {error}; inspect the installed DDEV version"
        ))
    })
}

fn parse_warnings(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
                && let Some(message) = value.get("msg").and_then(serde_json::Value::as_str)
            {
                return Some(message.to_owned());
            }
            Some(line.to_owned())
        })
        .collect()
}

fn canonical_or_raw(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn validate_hostname_part(value: &str) -> ToolResult<()> {
    if value.is_empty()
        || value.len() > 63
        || value.starts_with('-')
        || value.ends_with('-')
        || value
            .bytes()
            .any(|byte| !matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'))
    {
        return Err(ToolError::new(format!(
            "workspace resource key '{}' is not a safe DNS label",
            value
        )));
    }
    Ok(())
}

fn write_owned_asset(path: &Path, contents: &[u8]) -> ToolResult<bool> {
    if !asset_needs_update(path, contents)? {
        return Ok(false);
    }
    let temporary = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("asset"),
        std::process::id()
    ));
    let _temporary_guard = TemporaryAssetGuard {
        path: temporary.clone(),
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        OpenOptions::new().read(true).open(parent)?.sync_all()?;
    }
    Ok(true)
}

fn asset_needs_update(path: &Path, contents: &[u8]) -> ToolResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ToolError::new(format!(
                    "managed DDEV asset {} is not a regular file; refusing to overwrite it",
                    path.display()
                )));
            }
            let existing = fs::read(path)?;
            if existing == contents {
                return Ok(false);
            }
            if !String::from_utf8_lossy(&existing).contains(ASSET_MARKER) {
                return Err(ToolError::new(format!(
                    "DDEV asset {} already exists without the ddev-workspaces ownership marker; review it manually",
                    path.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(true)
}

fn ensure_directory(path: &Path, label: &str) -> ToolResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(ToolError::new(format!(
            "{label} {} is not a regular directory",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn ensure_regular_directory(path: &Path, label: &str) -> ToolResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ToolError::new(format!(
            "{label} {} cannot be inspected: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ToolError::new(format!(
            "{label} {} is not a regular directory",
            path.display()
        )));
    }
    Ok(())
}

fn set_executable(path: &Path) -> ToolResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn entrypoint_is_executable(path: &Path) -> ToolResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(ToolError::new(format!(
                "managed DDEV entrypoint {} is not a regular file",
                path.display()
            )))
        }
        Ok(_) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                Ok(fs::metadata(path)?.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                Ok(true)
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn create_symlink(target: &Path, link: &Path) -> ToolResult<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        let _ = link;
        Err(ToolError::new(
            "shared workspace routing requires a platform with symbolic links",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn hostname_is_resource_key_first() {
        assert_eq!(
            expected_url("fixture.ddev.site", "fixture-feature-test").unwrap(),
            "https://fixture-feature-test.fixture.ddev.site"
        );
    }

    #[test]
    fn json_output_parser_accepts_ddev_log_lines() {
        let output = "2026-01-01T00:00:00Z {\"msg\":\"log\"}\n{\"raw\":{\"global-ddev-dir\":\"/tmp/ddev\"}}\n";
        let parsed: DdevVersion = parse_json_output(output, "DDEV version").unwrap();
        assert_eq!(parsed.global_dir, PathBuf::from("/tmp/ddev"));
    }

    #[cfg(unix)]
    #[test]
    fn unpublish_removes_an_exact_dangling_route() {
        let directory = tempdir().unwrap();
        let links = state::links_directory(directory.path());
        fs::create_dir_all(&links).unwrap();
        let target = state::views_directory(directory.path()).join("fixture-feature");
        let link = links.join("fixture-feature");
        std::os::unix::fs::symlink(Path::new("../views/fixture-feature"), &link).unwrap();

        unpublish_link(directory.path(), "fixture-feature", &target).unwrap();
        assert!(fs::symlink_metadata(link).is_err());
    }
}
