use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::command::{CommandRequest, CommandRunner, ToolError, ToolResult};
use crate::config;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DdevProject {
    pub name: String,
    pub approot: String,
    pub status: String,
    #[serde(default)]
    pub mutagen_enabled: bool,
    #[serde(default)]
    pub mutagen_status: String,
}

#[derive(Debug, Deserialize)]
struct DdevListEnvelope {
    raw: Vec<DdevProject>,
}

#[derive(Debug, Deserialize)]
struct DdevVersionEnvelope {
    raw: DdevVersion,
}

#[derive(Debug, Deserialize)]
struct DdevVersion {
    #[serde(rename = "global-ddev-dir")]
    global_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct DdevOutputMessage {
    msg: String,
}

struct DdevListShadow {
    xdg_home: PathBuf,
}

impl Drop for DdevListShadow {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.xdg_home);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdevInspection {
    pub entries: Vec<DdevProject>,
    pub warnings: Vec<String>,
}

pub fn expected_name(project_id: &str, workspace_name: &str) -> ToolResult<String> {
    config::validate_name("project_id", project_id)?;
    config::validate_name("workspace name", workspace_name)?;
    let name = format!("dw-{project_id}--{workspace_name}");
    if name.len() > 63 {
        return Err(ToolError::new(format!(
            "DDEV project name `{name}` is longer than the 63-byte DNS-label limit"
        )));
    }
    Ok(name)
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
            "DDEV list failed; verify that DDEV is installed and retry the read-only diagnosis",
        ));
    }
    let envelope = serde_json::from_str::<DdevListEnvelope>(&output.stdout).map_err(|error| {
        ToolError::new(format!(
            "DDEV returned an unsupported JSON list envelope: {error}; upgrade guidance is required before mutation"
        ))
    })?;
    let mut warnings = output
        .stderr
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                None
            } else {
                Some(normalize_ddev_warning(
                    serde_json::from_str::<DdevOutputMessage>(line)
                        .map(|message| message.msg)
                        .unwrap_or_else(|_| line.to_owned()),
                ))
            }
        })
        .collect::<Vec<_>>();
    for entry in &envelope.raw {
        if !Path::new(&entry.approot).exists() {
            warnings.push(format!(
                "stale DDEV registration `{}` points to missing path {}",
                entry.name, entry.approot
            ));
        }
    }
    for left in 0..envelope.raw.len() {
        for right in left + 1..envelope.raw.len() {
            if envelope.raw[left].name == envelope.raw[right].name {
                warnings.push(format!(
                    "duplicate DDEV name `{}` is registered more than once",
                    envelope.raw[left].name
                ));
            }
            if canonical_or_raw(Path::new(&envelope.raw[left].approot))
                == canonical_or_raw(Path::new(&envelope.raw[right].approot))
            {
                warnings.push(format!(
                    "duplicate DDEV app root `{}` is registered under multiple names",
                    envelope.raw[left].approot
                ));
            }
        }
    }
    Ok(DdevInspection {
        entries: envelope.raw,
        warnings,
    })
}

fn normalize_ddev_warning(message: String) -> String {
    const PREFIX: &str = "The project '";
    const SUFFIX: &str = "' no longer exists in the filesystem, removing it from registry";
    if let Some(path) = message
        .strip_prefix(PREFIX)
        .and_then(|message| message.strip_suffix(SUFFIX))
    {
        format!("stale DDEV registration points to missing path {path}")
    } else {
        message
    }
}

fn prepare_list_shadow<R: CommandRunner>(runner: &mut R, cwd: &Path) -> ToolResult<DdevListShadow> {
    let version =
        runner.run(&CommandRequest::new("ddev", ["version", "--json-output"]).cwd(cwd))?;
    if !version.success() {
        return Err(ToolError::new(
            "DDEV version inspection failed; verify that DDEV is installed and retry the read-only diagnosis",
        ));
    }
    let envelope = serde_json::from_str::<DdevVersionEnvelope>(&version.stdout).map_err(|error| {
        ToolError::new(format!(
            "DDEV returned an unsupported JSON version envelope: {error}; upgrade guidance is required before inspection"
        ))
    })?;
    if !envelope.raw.global_dir.is_absolute() {
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
    let shadow_dir = xdg_home.join("ddev");
    fs::create_dir_all(&shadow_dir).map_err(|error| {
        ToolError::new(format!(
            "could not create isolated DDEV inspection directory: {error}"
        ))
    })?;
    let shadow = DdevListShadow { xdg_home };

    for filename in ["global_config.yaml", "project_list.yaml"] {
        let source = envelope.raw.global_dir.join(filename);
        if source.is_file() {
            fs::copy(&source, shadow_dir.join(filename)).map_err(|error| {
                ToolError::new(format!(
                    "could not isolate DDEV {filename} for read-only inspection: {error}"
                ))
            })?;
        }
    }
    #[cfg(unix)]
    {
        let source = envelope.raw.global_dir.join("bin");
        if source.is_dir() {
            std::os::unix::fs::symlink(source, shadow_dir.join("bin")).map_err(|error| {
                ToolError::new(format!(
                    "could not expose DDEV helper binaries to isolated inspection: {error}"
                ))
            })?;
        }
    }

    Ok(shadow)
}

pub fn inspect_new_identity(
    inspection: &DdevInspection,
    expected_name: &str,
    app_root: &Path,
) -> ToolResult<()> {
    let expected_path = canonical_or_raw(app_root);
    for entry in &inspection.entries {
        let entry_path = canonical_or_raw(Path::new(&entry.approot));
        if entry.name == expected_name {
            return Err(ToolError::new(format!(
                "DDEV project name `{expected_name}` is already registered at {}; v1 never adopts it",
                entry.approot
            )));
        }
        if entry_path == expected_path {
            return Err(ToolError::new(format!(
                "workspace app root {} is already registered in DDEV as `{}`; v1 never reuses another identity",
                app_root.display(),
                entry.name
            )));
        }
    }
    Ok(())
}

pub fn require_ready_identity(
    inspection: &DdevInspection,
    expected_name: &str,
    app_root: &Path,
) -> ToolResult<DdevProject> {
    let expected_path = canonical_or_raw(app_root);
    let matches = inspection
        .entries
        .iter()
        .filter(|entry| {
            entry.name == expected_name
                || canonical_or_raw(Path::new(&entry.approot)) == expected_path
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(ToolError::new(format!(
            "DDEV identity `{expected_name}` is not unique for {}; refusing readiness",
            app_root.display()
        )));
    }
    let entry = matches[0];
    if entry.name != expected_name || canonical_or_raw(Path::new(&entry.approot)) != expected_path {
        return Err(ToolError::new(format!(
            "DDEV identity mismatch: expected `{expected_name}` at {}, found `{}` at {}",
            app_root.display(),
            entry.name,
            entry.approot
        )));
    }
    if !entry.status.eq_ignore_ascii_case("running") {
        return Err(ToolError::new(format!(
            "DDEV project `{expected_name}` is not running (status `{}`); rerun doctor after manual remediation",
            entry.status
        )));
    }
    if entry.mutagen_enabled && !entry.mutagen_status.eq_ignore_ascii_case("ok") {
        return Err(ToolError::new(format!(
            "DDEV Mutagen for `{expected_name}` is not healthy (status `{}`); repair it manually",
            entry.mutagen_status
        )));
    }
    Ok(entry.clone())
}

pub fn write_override<R: CommandRunner>(
    repo_root: &Path,
    app_root: &Path,
    expected_name: &str,
    disable_mutagen: bool,
    runner: &mut R,
) -> ToolResult<PathBuf> {
    let ddev_directory = app_root.join(".ddev");
    match fs::symlink_metadata(&ddev_directory) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
        Ok(_) => {
            return Err(ToolError::new(format!(
                "DDEV directory {} must be a regular directory; refusing to write outside the workspace",
                ddev_directory.display()
            )));
        }
        Err(_) => {
            return Err(ToolError::new(format!(
                "DDEV app root {} is missing named `.ddev/config.yaml`; materialize that file explicitly before start",
                app_root.display()
            )));
        }
    }
    let config_path = ddev_directory.join("config.yaml");
    if !is_regular_file(&config_path) {
        return Err(ToolError::new(format!(
            "DDEV app root {} is missing named `.ddev/config.yaml`; materialize that file explicitly before start",
            app_root.display()
        )));
    }
    let override_path = ddev_directory.join("config.ddev-workspaces.yaml");
    if fs::symlink_metadata(&override_path).is_ok() {
        return Err(ToolError::new(format!(
            "DDEV override {} already exists; v1 never overwrites local configuration",
            override_path.display()
        )));
    }
    if override_path.starts_with(repo_root)
        && !config::is_ignored(repo_root, &override_path, runner)?
    {
        return Err(ToolError::new(format!(
            "DDEV override {} is not ignored by Git; add a local ignore rule manually",
            override_path.display()
        )));
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&override_path)
        .map_err(|error| {
            ToolError::new(format!(
                "cannot create owned DDEV override {}: {error}",
                override_path.display()
            ))
        })?;
    use std::io::Write;
    let mut contents = format!("name: {expected_name}\n");
    if disable_mutagen {
        contents.push_str("performance_mode: none\n");
    }
    file.write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            ToolError::new(format!(
                "cannot finish owned DDEV override {}: {error}",
                override_path.display()
            ))
        })?;
    Ok(override_path)
}

pub fn prepare_source_site(
    workspace: &Path,
    app_root: &Path,
    generated_root: &Path,
    source_root: &Path,
    repository_path: &str,
) -> ToolResult<()> {
    reserve_generated_app_root(generated_root, app_root)?;
    let excluded_repository = source_root.join(repository_path);
    copy_source_site_tree(source_root, app_root, &excluded_repository, source_root)?;
    let mounted_repository = app_root.join(repository_path);
    fs::create_dir_all(&mounted_repository)?;
    let ddev_directory = app_root.join(".ddev");
    let repository = serde_json::to_string(&format!(
        "{}:/var/www/html/{}",
        workspace.display(),
        repository_path
    ))
    .map_err(|error| ToolError::new(format!("cannot encode repository mount: {error}")))?;
    let compose = format!("services:\n  web:\n    volumes:\n      - {repository}\n");
    let compose_path = ddev_directory.join("docker-compose.ddev-workspaces.yaml");
    let mut compose_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&compose_path)
        .map_err(|error| {
            ToolError::new(format!(
                "cannot create owned DDEV compose file {}: {error}",
                compose_path.display()
            ))
        })?;
    compose_file.write_all(compose.as_bytes())?;
    compose_file.sync_all()?;
    Ok(())
}

fn reserve_generated_app_root(generated_root: &Path, app_root: &Path) -> ToolResult<()> {
    let generated_root = fs::canonicalize(generated_root)?;
    let relative = app_root.strip_prefix(&generated_root).map_err(|_| {
        ToolError::new(format!(
            "generated DDEV app root {} is outside generated root {}",
            app_root.display(),
            generated_root.display()
        ))
    })?;
    let mut current = generated_root;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(part) = component else {
            return Err(ToolError::new(
                "generated DDEV app root contains an unsafe component",
            ));
        };
        current.push(part);
        let is_app_root = components.peek().is_none();
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && !is_app_root => {
                let metadata = fs::symlink_metadata(&current)?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(ToolError::new(format!(
                        "generated DDEV parent {} is not a regular directory",
                        current.display()
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(ToolError::new(format!(
                    "generated DDEV app root {} already exists; refusing to overwrite it",
                    app_root.display()
                )));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn copy_source_site_tree(
    source: &Path,
    destination: &Path,
    excluded_repository: &Path,
    source_root: &Path,
) -> ToolResult<()> {
    if source == excluded_repository {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)?;
        let resolved_target = source
            .parent()
            .ok_or_else(|| ToolError::new("source DDEV site symlink has no parent"))?
            .join(&target);
        let canonical_target = fs::canonicalize(&resolved_target).map_err(|error| {
            ToolError::new(format!(
                "source DDEV site symlink {} has an unavailable target: {error}",
                source.display()
            ))
        })?;
        if target.is_absolute()
            || !canonical_target.starts_with(source_root)
            || canonical_target.starts_with(excluded_repository)
        {
            return Err(ToolError::new(format!(
                "source DDEV site contains symlink {}; refusing to copy a link that could escape the generated site",
                source.display()
            )));
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination)?;
        #[cfg(not(unix))]
        return Err(ToolError::new(format!(
            "source DDEV site contains symlink {}, which is unsupported on this platform",
            source.display()
        )));
        return Ok(());
    }
    if metadata.is_file() {
        let mut input = fs::File::open(source)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        fs::set_permissions(destination, metadata.permissions())?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(ToolError::new(format!(
            "source DDEV site contains unsupported entry {}",
            source.display()
        )));
    }
    if source != source_root {
        fs::create_dir(destination)?;
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let child_source = entry.path();
        if child_source == excluded_repository {
            continue;
        }
        let name = child_source.file_name().and_then(|name| name.to_str());
        let relative = child_source
            .strip_prefix(source_root)
            .unwrap_or(&child_source);
        let in_ddev = relative.starts_with(".ddev");
        if name.is_some_and(|name| {
            matches!(
                name,
                ".git"
                    | "node_modules"
                    | "config.ddev-workspaces.yaml"
                    | "config.ddev-workspaces-export.yaml"
                    | "docker-compose.ddev-workspaces.yaml"
            ) || (in_ddev
                && matches!(
                    name,
                    ".ddev-docker-compose-base.yaml"
                        | ".ddev-docker-compose-full.yaml"
                        | ".dbimageBuild"
                        | ".webimageBuild"
                        | ".homeadditions"
                        | ".start-synced"
                        | "db_snapshots"
                        | "traefik"
                ))
        }) {
            continue;
        }
        copy_source_site_tree(
            &child_source,
            &destination.join(entry.file_name()),
            excluded_repository,
            source_root,
        )?;
    }
    fs::set_permissions(destination, metadata.permissions())?;
    Ok(())
}

pub fn start<R: CommandRunner>(app_root: &Path, runner: &mut R) -> ToolResult<()> {
    let request = CommandRequest::new("ddev", ["start"])
        .cwd(app_root)
        .mutating();
    let output = runner.run(&request)?;
    if output.success() {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "DDEV start failed for {}; preserve the owned workspace and inspect DDEV manually",
            app_root.display()
        )))
    }
}

struct DatabaseDump {
    directory: PathBuf,
    path: PathBuf,
}

impl DatabaseDump {
    fn create() -> ToolResult<Self> {
        let temp_root = std::env::temp_dir();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| ToolError::new(format!("system clock is unavailable: {error}")))?
            .as_nanos();
        for attempt in 0..32u8 {
            let directory = temp_root.join(format!(
                "ddev-workspaces-database-{}-{nonce}-{attempt}",
                std::process::id()
            ));
            let result = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    let mut builder = fs::DirBuilder::new();
                    builder.mode(0o700).create(&directory)
                }
                #[cfg(not(unix))]
                {
                    fs::create_dir(&directory)
                }
            };
            match result {
                Ok(()) => {
                    return Ok(Self {
                        path: directory.join("database.sql.gz"),
                        directory,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ToolError::new(
            "could not reserve a private temporary directory for the database clone",
        ))
    }
}

struct SourceMutagenOverride {
    path: PathBuf,
}

impl SourceMutagenOverride {
    fn create(source_root: &Path) -> ToolResult<Self> {
        let path = source_root
            .join(".ddev")
            .join("config.ddev-workspaces-export.yaml");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                ToolError::new(format!(
                    "cannot create temporary source DDEV override {}: {error}",
                    path.display()
                ))
            })?;
        let override_file = Self { path };
        use std::io::Write;
        file.write_all(b"performance_mode: none\n")
            .and_then(|_| file.sync_all())?;
        Ok(override_file)
    }
}

impl Drop for SourceMutagenOverride {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl Drop for DatabaseDump {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(unix)]
struct InterruptGuard {
    interrupted: Arc<AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}

#[cfg(unix)]
impl InterruptGuard {
    fn create() -> ToolResult<Self> {
        let interrupted = Arc::new(AtomicBool::new(false));
        let registrations = [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM]
            .into_iter()
            .map(|signal| signal_hook::flag::register(signal, Arc::clone(&interrupted)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                ToolError::new(format!("cannot install cleanup signal handler: {error}"))
            })?;
        Ok(Self {
            interrupted,
            registrations,
        })
    }

    fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Relaxed)
    }
}

#[cfg(unix)]
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

#[cfg(not(unix))]
struct InterruptGuard;

#[cfg(not(unix))]
impl InterruptGuard {
    fn create() -> ToolResult<Self> {
        Ok(Self)
    }

    fn interrupted(&self) -> bool {
        false
    }
}

pub fn clone_database<R: CommandRunner>(
    source_root: &Path,
    target_root: &Path,
    runner: &mut R,
) -> ToolResult<()> {
    let interrupt_guard = InterruptGuard::create()?;
    let source_was_running = list(runner, source_root)?.entries.iter().any(|project| {
        canonical_or_raw(Path::new(&project.approot)) == canonical_or_raw(source_root)
            && project.status.eq_ignore_ascii_case("running")
    });
    let _source_override = if source_was_running {
        None
    } else {
        Some(SourceMutagenOverride::create(source_root)?)
    };
    let dump = DatabaseDump::create()?;
    let clone_result = (|| {
        let export = CommandRequest::new(
            "ddev",
            [
                "export-db".to_owned(),
                format!("--file={}", dump.path.display()),
            ],
        )
        .cwd(source_root)
        .mutating();
        if !runner.run(&export)?.success() {
            return Err(ToolError::new(format!(
                "could not export the source DDEV database from {}",
                source_root.display()
            )));
        }
        let import = CommandRequest::new(
            "ddev",
            [
                "import-db".to_owned(),
                "--no-progress".to_owned(),
                format!("--file={}", dump.path.display()),
            ],
        )
        .cwd(target_root)
        .mutating();
        let output = runner.run(&import)?;
        if !output.success() {
            let detail = format!("{}{}", output.stdout, output.stderr);
            let detail = detail.trim();
            return Err(ToolError::new(format!(
                "could not import the source DDEV database into {}{}",
                target_root.display(),
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            )));
        }
        Ok(())
    })();
    let restore_result = if source_was_running {
        Ok(())
    } else {
        let stop = CommandRequest::new("ddev", ["stop"])
            .cwd(source_root)
            .mutating();
        if runner.run(&stop)?.success() {
            Ok(())
        } else {
            Err(ToolError::new(format!(
                "source DDEV database was cloned, but {} could not be restored to its stopped state",
                source_root.display()
            )))
        }
    };
    let interrupted = interrupt_guard.interrupted();
    match (clone_result, restore_result, interrupted) {
        (Err(clone_error), Err(restore_error), _) => Err(ToolError::new(format!(
            "{clone_error}; additionally, source-state restoration failed: {restore_error}"
        ))),
        (_, Err(restore_error), _) => Err(restore_error),
        (Err(clone_error), Ok(()), _) => Err(clone_error),
        (Ok(()), Ok(()), true) => Err(ToolError::new(
            "database cloning was interrupted after source-state cleanup completed",
        )),
        (Ok(()), Ok(()), false) => Ok(()),
    }
}

pub fn stop_unlist<R: CommandRunner>(
    app_root: &Path,
    name: &str,
    delete_data: bool,
    runner: &mut R,
) -> ToolResult<()> {
    let args = if delete_data {
        vec![
            "stop".to_owned(),
            "--remove-data".to_owned(),
            "--unlist".to_owned(),
            name.to_owned(),
        ]
    } else {
        vec!["stop".to_owned(), "--unlist".to_owned(), name.to_owned()]
    };
    let request = CommandRequest::new("ddev", args).cwd(app_root).mutating();
    let output = runner.run(&request)?;
    if output.success() {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "DDEV refused to stop and unlist `{name}`; no Git or ownership mutation was attempted"
        )))
    }
}

fn canonical_or_raw(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandOutput, CommandRunner};
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[derive(Default)]
    struct FakeRunner {
        requests: Vec<CommandRequest>,
        responses: Vec<CommandOutput>,
    }

    impl CommandRunner for FakeRunner {
        fn run(&mut self, request: &CommandRequest) -> ToolResult<CommandOutput> {
            self.requests.push(request.clone());
            Ok(self.responses.pop().unwrap_or_else(|| CommandOutput {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            }))
        }
    }

    #[test]
    fn names_are_deterministic_and_limited_to_dns_label_size() {
        let name = expected_name("fixture", "invoice-fix").expect("valid DDEV name");

        assert_eq!(name, "dw-fixture--invoice-fix");
        assert!(expected_name(&"a".repeat(60), "task").is_err());
    }

    #[test]
    fn exact_identity_requires_running_and_healthy_mutagen() {
        let inspection = DdevInspection {
            entries: vec![DdevProject {
                name: "dw-fixture--task".to_owned(),
                approot: "/tmp/workspace".to_owned(),
                status: "running".to_owned(),
                mutagen_enabled: true,
                mutagen_status: "ok".to_owned(),
            }],
            warnings: Vec::new(),
        };

        let project = require_ready_identity(
            &inspection,
            "dw-fixture--task",
            &PathBuf::from("/tmp/workspace"),
        )
        .expect("identity should be ready");

        assert_eq!(project.name, "dw-fixture--task");
    }

    #[test]
    fn conflicting_name_or_path_is_rejected_before_start() {
        let inspection = DdevInspection {
            entries: vec![DdevProject {
                name: "dw-fixture--other".to_owned(),
                approot: "/tmp/workspace".to_owned(),
                status: "running".to_owned(),
                mutagen_enabled: false,
                mutagen_status: String::new(),
            }],
            warnings: Vec::new(),
        };

        let error = inspect_new_identity(
            &inspection,
            "dw-fixture--task",
            &PathBuf::from("/tmp/workspace"),
        )
        .unwrap_err();

        assert!(error.to_string().contains("already registered"));
    }

    #[test]
    fn malformed_json_fails_closed_without_using_partial_state() {
        let global_directory = tempdir().expect("fake DDEV global directory");
        let mut runner = FakeRunner {
            requests: Vec::new(),
            responses: vec![
                CommandOutput {
                    status: 0,
                    stdout: "{not-json".to_owned(),
                    stderr: String::new(),
                },
                CommandOutput {
                    status: 0,
                    stdout: format!(
                        "{{\"raw\":{{\"global-ddev-dir\":\"{}\"}}}}",
                        global_directory.path().display()
                    ),
                    stderr: String::new(),
                },
            ],
        };

        let error = list(&mut runner, Path::new("/tmp")).unwrap_err();

        assert!(error.to_string().contains("unsupported JSON list envelope"));
        assert_eq!(runner.requests.len(), 2);
        assert_eq!(runner.requests[0].args, ["version", "--json-output"]);
        assert_eq!(runner.requests[1].args, ["list", "--json-output"]);
    }

    #[test]
    fn data_removal_uses_only_stop_remove_data_unlist() {
        let mut runner = FakeRunner::default();

        stop_unlist(
            Path::new("/tmp/workspace"),
            "dw-fixture--task",
            true,
            &mut runner,
        )
        .expect("fake stop should succeed");

        assert_eq!(
            runner.requests[0].args,
            vec![
                "stop".to_owned(),
                "--remove-data".to_owned(),
                "--unlist".to_owned(),
                "dw-fixture--task".to_owned()
            ]
        );
        assert!(!runner.requests[0].args.iter().any(|arg| arg == "delete"));
    }

    #[test]
    fn list_is_read_only_and_reports_registry_warnings() {
        let directory = tempdir().expect("temporary directory");
        let global = directory.path().join("global");
        let existing_app = directory.path().join("app");
        let missing_app = directory.path().join("missing");
        fs::create_dir_all(global.join("bin")).expect("global directory");
        fs::create_dir(&existing_app).expect("existing app root");
        fs::write(
            global.join("global_config.yaml"),
            "instrumentation_opt_in: false\n",
        )
        .expect("global config");
        fs::write(global.join("project_list.yaml"), "project_info: {}\n").expect("project list");
        let list_json = format!(
            "{{\"raw\":[{{\"name\":\"one\",\"approot\":\"{}\",\"status\":\"running\"}},{{\"name\":\"one\",\"approot\":\"{}\",\"status\":\"stopped\"}},{{\"name\":\"three\",\"approot\":\"{}\",\"status\":\"running\"}}]}}",
            existing_app.display(),
            missing_app.display(),
            existing_app.display()
        );
        let mut runner = FakeRunner {
            requests: Vec::new(),
            responses: vec![
                CommandOutput {
                    status: 0,
                    stdout: list_json,
                    stderr: format!(
                        "{{\"msg\":\"The project '{}' no longer exists in the filesystem, removing it from registry\"}}\nplain warning\n",
                        missing_app.display()
                    ),
                },
                CommandOutput {
                    status: 0,
                    stdout: format!(
                        "{{\"raw\":{{\"global-ddev-dir\":\"{}\"}}}}",
                        global.display()
                    ),
                    stderr: String::new(),
                },
            ],
        };

        let inspection = list(&mut runner, directory.path()).expect("DDEV inspection");

        assert_eq!(inspection.entries.len(), 3);
        assert!(
            inspection
                .warnings
                .iter()
                .any(|warning| warning == "plain warning")
        );
        assert!(
            inspection
                .warnings
                .iter()
                .any(|warning| warning.contains("duplicate DDEV name `one`"))
        );
        assert!(
            inspection
                .warnings
                .iter()
                .any(|warning| warning.contains("duplicate DDEV app root"))
        );
        assert!(
            inspection
                .warnings
                .iter()
                .any(|warning| warning.contains("stale DDEV registration"))
        );
        let shadow = runner.requests[1]
            .env
            .iter()
            .find(|(name, _)| name == "DDEV_XDG_CONFIG_HOME")
            .expect("isolated DDEV configuration");
        assert!(!Path::new(&shadow.1).exists());
        assert!(!runner.requests.iter().any(|request| request.mutating));
    }

    #[test]
    fn list_rejects_failed_or_unsafe_version_and_list_inspection() {
        let directory = tempdir().expect("temporary directory");
        let cases = [
            CommandOutput {
                status: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
            CommandOutput {
                status: 0,
                stdout: "{not-json".to_owned(),
                stderr: String::new(),
            },
            CommandOutput {
                status: 0,
                stdout: "{\"raw\":{\"global-ddev-dir\":\"relative\"}}".to_owned(),
                stderr: String::new(),
            },
        ];
        for response in cases {
            let mut runner = FakeRunner {
                requests: Vec::new(),
                responses: vec![response],
            };
            assert!(list(&mut runner, directory.path()).is_err());
            assert_eq!(runner.requests.len(), 1);
        }

        let mut runner = FakeRunner {
            requests: Vec::new(),
            responses: vec![
                CommandOutput {
                    status: 1,
                    stdout: String::new(),
                    stderr: String::new(),
                },
                CommandOutput {
                    status: 0,
                    stdout: format!(
                        "{{\"raw\":{{\"global-ddev-dir\":\"{}\"}}}}",
                        directory.path().display()
                    ),
                    stderr: String::new(),
                },
            ],
        };
        assert!(list(&mut runner, directory.path()).is_err());
        assert_eq!(runner.requests.len(), 2);
    }

    #[test]
    fn readiness_requires_one_exact_running_healthy_identity() {
        let root = PathBuf::from("/tmp/workspace");
        let project = |name: &str, approot: &str, status: &str, mutagen_status: &str| DdevProject {
            name: name.to_owned(),
            approot: approot.to_owned(),
            status: status.to_owned(),
            mutagen_enabled: !mutagen_status.is_empty(),
            mutagen_status: mutagen_status.to_owned(),
        };

        let cases = [
            vec![],
            vec![project("other", "/tmp/workspace", "running", "")],
            vec![project("dw-fixture--task", "/tmp/workspace", "stopped", "")],
            vec![project(
                "dw-fixture--task",
                "/tmp/workspace",
                "running",
                "failed",
            )],
            vec![
                project("dw-fixture--task", "/tmp/workspace", "running", ""),
                project("dw-fixture--task", "/tmp/other", "running", ""),
            ],
        ];
        for entries in cases {
            let inspection = DdevInspection {
                entries,
                warnings: Vec::new(),
            };
            assert!(require_ready_identity(&inspection, "dw-fixture--task", &root).is_err());
        }
    }

    #[test]
    fn override_creation_requires_named_regular_ignored_ddev_configuration() {
        let repository = tempdir().expect("repository");
        let app = repository.path().join("app");
        fs::create_dir(&app).expect("app root");
        let mut runner = FakeRunner::default();
        assert!(
            write_override(
                repository.path(),
                &app,
                "dw-fixture--task",
                false,
                &mut runner
            )
            .is_err()
        );

        fs::create_dir(app.join(".ddev")).expect("DDEV directory");
        assert!(
            write_override(
                repository.path(),
                &app,
                "dw-fixture--task",
                false,
                &mut runner
            )
            .is_err()
        );

        fs::write(app.join(".ddev/config.yaml"), "type: php\n").expect("DDEV config");
        runner.responses.push(CommandOutput {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        });
        let override_path = write_override(
            repository.path(),
            &app,
            "dw-fixture--task",
            false,
            &mut runner,
        )
        .expect("ignored override should be created");
        assert_eq!(
            fs::read_to_string(&override_path).expect("override contents"),
            "name: dw-fixture--task\n"
        );
        assert!(
            write_override(
                repository.path(),
                &app,
                "dw-fixture--task",
                false,
                &mut runner
            )
            .is_err()
        );
    }

    #[test]
    fn start_and_stop_fail_closed_on_nonzero_process_status() {
        let failure = || CommandOutput {
            status: 1,
            stdout: String::new(),
            stderr: "failed".to_owned(),
        };
        let mut runner = FakeRunner {
            requests: Vec::new(),
            responses: vec![failure(), failure()],
        };

        assert!(start(Path::new("/tmp/workspace"), &mut runner).is_err());
        assert!(
            stop_unlist(
                Path::new("/tmp/workspace"),
                "dw-fixture--task",
                false,
                &mut runner
            )
            .is_err()
        );
        assert!(runner.requests.iter().all(|request| request.mutating));
    }

    #[cfg(unix)]
    #[test]
    fn source_site_copy_rejects_symlinks_before_owned_files_are_written() {
        use std::os::unix::fs::symlink;

        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let generated = fixture.path().join("generated");
        let source_ddev = fixture.path().join("source-ddev");
        fs::create_dir_all(&source).expect("source");
        fs::create_dir(&generated).expect("generated root");
        let generated = fs::canonicalize(generated).expect("canonical generated root");
        let app_root = generated.join("fixture/task");
        fs::create_dir(&source_ddev).expect("source DDEV directory");
        fs::write(source_ddev.join("config.yaml"), "type: wordpress\n").expect("DDEV config");
        symlink(&source_ddev, source.join(".ddev")).expect("source DDEV symlink");

        let error = prepare_source_site(
            fixture.path(),
            &app_root,
            &generated,
            &source,
            "wp-content/plugins/fixture",
        )
        .unwrap_err();

        assert!(error.to_string().contains("contains symlink"), "{error}");
        assert!(
            !source_ddev
                .join("docker-compose.ddev-workspaces.yaml")
                .exists()
        );
    }

    #[test]
    fn source_site_reserves_the_app_root_exclusively() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let generated = fixture.path().join("generated");
        fs::create_dir_all(source.join(".ddev")).expect("source DDEV directory");
        fs::write(source.join(".ddev/config.yaml"), "type: wordpress\n").expect("DDEV config");
        fs::create_dir(&generated).expect("generated root");
        let generated = fs::canonicalize(generated).expect("canonical generated root");
        let app_root = generated.join("fixture/task");
        fs::create_dir_all(&app_root).expect("occupied app root");

        let error = prepare_source_site(
            fixture.path(),
            &app_root,
            &generated,
            &source,
            "wp-content/plugins/fixture",
        )
        .unwrap_err();

        assert!(error.to_string().contains("already exists"), "{error}");
    }

    #[test]
    fn database_clone_reports_clone_and_restoration_failures_and_cleans_private_dump() {
        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let target = fixture.path().join("target");
        let global = fixture.path().join("global");
        fs::create_dir_all(source.join(".ddev")).expect("source DDEV directory");
        fs::create_dir(&target).expect("target");
        fs::create_dir(&global).expect("global");
        let failed = CommandOutput {
            status: 1,
            stdout: String::new(),
            stderr: String::new(),
        };
        let mut runner = FakeRunner {
            requests: Vec::new(),
            responses: vec![
                failed.clone(),
                failed,
                CommandOutput {
                    status: 0,
                    stdout: "{\"raw\":[]}".to_owned(),
                    stderr: String::new(),
                },
                CommandOutput {
                    status: 0,
                    stdout: format!(
                        "{{\"raw\":{{\"global-ddev-dir\":\"{}\"}}}}",
                        global.display()
                    ),
                    stderr: String::new(),
                },
            ],
        };

        let error = clone_database(&source, &target, &mut runner).unwrap_err();
        let export = runner
            .requests
            .iter()
            .find(|request| request.args.first().is_some_and(|arg| arg == "export-db"))
            .expect("export request");
        let dump = export
            .args
            .iter()
            .find_map(|argument| argument.strip_prefix("--file="))
            .map(PathBuf::from)
            .expect("database dump path");

        assert!(error.to_string().contains("could not export"));
        assert!(
            error
                .to_string()
                .contains("source-state restoration failed")
        );
        assert!(!dump.parent().expect("private dump directory").exists());
        assert!(
            !source
                .join(".ddev/config.ddev-workspaces-export.yaml")
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn database_clone_restores_a_stopped_source_after_an_interrupt() {
        struct InterruptingRunner {
            global: PathBuf,
            requests: Vec<CommandRequest>,
        }

        impl CommandRunner for InterruptingRunner {
            fn run(&mut self, request: &CommandRequest) -> ToolResult<CommandOutput> {
                self.requests.push(request.clone());
                let command = request.args.first().map(String::as_str);
                match command {
                    Some("version") => Ok(CommandOutput {
                        status: 0,
                        stdout: format!(
                            "{{\"raw\":{{\"global-ddev-dir\":\"{}\"}}}}",
                            self.global.display()
                        ),
                        stderr: String::new(),
                    }),
                    Some("list") => Ok(CommandOutput {
                        status: 0,
                        stdout: "{\"raw\":[]}".to_owned(),
                        stderr: String::new(),
                    }),
                    Some("export-db") => {
                        signal_hook::low_level::raise(signal_hook::consts::SIGINT)
                            .expect("raise SIGINT");
                        Ok(CommandOutput {
                            status: 1,
                            stdout: String::new(),
                            stderr: "interrupted".to_owned(),
                        })
                    }
                    Some("stop") => Ok(CommandOutput {
                        status: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    }),
                    _ => unreachable!("unexpected DDEV request"),
                }
            }
        }

        let fixture = tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let target = fixture.path().join("target");
        let global = fixture.path().join("global");
        fs::create_dir_all(source.join(".ddev")).expect("source DDEV directory");
        fs::create_dir(&target).expect("target");
        fs::create_dir(&global).expect("global");
        let mut runner = InterruptingRunner {
            global,
            requests: Vec::new(),
        };

        let error = clone_database(&source, &target, &mut runner).unwrap_err();

        assert!(error.to_string().contains("could not export"));
        assert!(
            runner
                .requests
                .iter()
                .any(|request| request.args.first().is_some_and(|arg| arg == "stop"))
        );
        assert!(
            !source
                .join(".ddev/config.ddev-workspaces-export.yaml")
                .exists()
        );
    }
}
