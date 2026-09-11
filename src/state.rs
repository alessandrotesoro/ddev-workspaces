use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::command::{ToolError, ToolResult};

pub const STATE_DIRECTORY: &str = ".ddev-workspaces";
pub const RECORDS_DIRECTORY: &str = "workspaces";
pub const LOCKS_DIRECTORY: &str = "locks";
pub const LINKS_DIRECTORY: &str = "links";
pub const VIEWS_DIRECTORY: &str = "views";
pub const RESTART_MARKER: &str = ".restart-required";
pub const RECORD_VERSION: u32 = 2;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceStatus {
    Creating,
    Ready,
    Error,
    Removing,
    RetainedData,
}

impl WorkspaceStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Error => "error",
            Self::Removing => "removing",
            Self::RetainedData => "retained-data",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseProvenance {
    Copy,
    Fresh,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DatabaseRecord {
    pub name: String,
    pub provenance: DatabaseProvenance,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub prepared: bool,
    #[serde(default)]
    pub ownership_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnedPaths {
    pub link: String,
    #[serde(default)]
    pub view: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub writable: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnershipRecord {
    pub version: u32,
    pub project_id: String,
    pub workspace_name: String,
    pub resource_key: String,
    pub adapter: String,
    pub repository_root: String,
    pub common_directory: String,
    pub host_root: String,
    pub worktree_path: String,
    pub base_sha: String,
    pub branch: String,
    pub hostname: String,
    pub url: String,
    pub database: DatabaseRecord,
    pub paths: OwnedPaths,
    pub status: WorkspaceStatus,
    #[serde(default)]
    pub error: Option<String>,
}

impl OwnershipRecord {
    pub fn is_retained_data(&self) -> bool {
        matches!(self.status, WorkspaceStatus::RetainedData)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordEntry {
    pub name: String,
    pub record: Result<OwnershipRecord, String>,
}

pub struct FileLock {
    file: File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub fn state_root(host_root: &Path) -> PathBuf {
    host_root.join(STATE_DIRECTORY)
}

pub fn records_directory(host_root: &Path) -> PathBuf {
    state_root(host_root).join(RECORDS_DIRECTORY)
}

pub fn locks_directory(host_root: &Path) -> PathBuf {
    state_root(host_root).join(LOCKS_DIRECTORY)
}

pub fn links_directory(host_root: &Path) -> PathBuf {
    state_root(host_root).join(LINKS_DIRECTORY)
}

pub fn views_directory(host_root: &Path) -> PathBuf {
    state_root(host_root).join(VIEWS_DIRECTORY)
}

pub fn record_path(host_root: &Path, resource_key: &str) -> PathBuf {
    records_directory(host_root).join(format!("{resource_key}.toml"))
}

pub fn ensure_state(host_root: &Path) -> ToolResult<()> {
    ensure_directory(&state_root(host_root), "host-local state directory")?;
    ensure_directory(&records_directory(host_root), "workspace record directory")?;
    ensure_directory(&locks_directory(host_root), "workspace lock directory")?;
    ensure_directory(&links_directory(host_root), "workspace link directory")?;
    ensure_directory(&views_directory(host_root), "workspace view directory")?;
    let ignore = state_root(host_root).join(".gitignore");
    match fs::symlink_metadata(&ignore) {
        Ok(metadata) if metadata.file_type().is_file() => {
            validate_state_ignore(&ignore)?;
        }
        Ok(_) => {
            return Err(ToolError::new(format!(
                "host-local state ignore file {} is not a regular file",
                ignore.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let result = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&ignore);
            match result {
                Ok(mut file) => {
                    set_private(&file)?;
                    file.write_all(b"*\n!.gitignore\n")?;
                    file.sync_all()?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    ensure_regular_file(&ignore, "host-local state ignore file")?;
                    validate_state_ignore(&ignore)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn validate_state_ignore(path: &Path) -> ToolResult<()> {
    let contents = fs::read_to_string(path)?;
    if contents != "*\n!.gitignore\n" {
        return Err(ToolError::new(format!(
            "host-local state ignore file {} does not match the CLI-owned contract; inspect it manually",
            path.display()
        )));
    }
    Ok(())
}

pub fn restart_required(host_root: &Path) -> ToolResult<bool> {
    let path = state_root(host_root).join(RESTART_MARKER);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(ToolError::new(format!(
            "host restart marker {} is not a regular file",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub fn mark_restart_required(host_root: &Path) -> ToolResult<()> {
    ensure_state(host_root)?;
    let path = state_root(host_root).join(RESTART_MARKER);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            set_private(&file)?;
            file.write_all(b"ddev-workspaces\n")?;
            file.sync_all()?;
            sync_directory(path.parent())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure_regular_file(&path, "host restart marker")
        }
        Err(error) => Err(error.into()),
    }
}

pub fn clear_restart_required(host_root: &Path) -> ToolResult<()> {
    let path = state_root(host_root).join(RESTART_MARKER);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::remove_file(&path)?;
            sync_directory(path.parent())
        }
        Ok(_) => Err(ToolError::new(format!(
            "host restart marker {} is not a regular file",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn acquire_host_lock(host_root: &Path) -> ToolResult<FileLock> {
    acquire_lock(host_root, "host")
}

pub fn acquire_workspace_lock(host_root: &Path, resource_key: &str) -> ToolResult<FileLock> {
    acquire_lock(host_root, resource_key)
}

fn acquire_lock(host_root: &Path, name: &str) -> ToolResult<FileLock> {
    ensure_state(host_root)?;
    validate_lock_name(name)?;
    let path = locks_directory(host_root).join(format!("{name}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| {
            ToolError::new(format!(
                "cannot open workspace lock {}: {error}",
                path.display()
            ))
        })?;
    match file.try_lock() {
        Ok(()) => Ok(FileLock { file }),
        Err(std::fs::TryLockError::WouldBlock) => Err(ToolError::new(format!(
            "workspace operation is already in progress; lock {} is held by another process",
            path.display()
        ))),
        Err(std::fs::TryLockError::Error(error)) => Err(ToolError::new(format!(
            "cannot acquire workspace lock {}: {error}",
            path.display()
        ))),
    }
}

pub fn reserve(host_root: &Path, record: &OwnershipRecord) -> ToolResult<PathBuf> {
    ensure_state(host_root)?;
    validate_record_key(&record.resource_key)?;
    let path = record_path(host_root, &record.resource_key);
    let contents = toml::to_string(record)?;
    let temporary = path.with_file_name(format!(
        ".{}.reserve-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("record"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                ToolError::new(format!(
                    "workspace reservation staging file {} already exists; refusing to continue",
                    temporary.display()
                ))
            } else {
                ToolError::new(format!(
                    "cannot stage ownership record {}: {error}",
                    temporary.display()
                ))
            }
        })?;
    set_private(&file)?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(ToolError::new(format!(
            "initial ownership reservation failed at {}: {error}",
            temporary.display()
        )));
    }
    drop(file);
    if let Err(error) = fs::hard_link(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(ToolError::new(format!(
                "workspace resource {} is already reserved at {}; refusing to adopt it",
                record.resource_key,
                path.display()
            )));
        }
        return Err(ToolError::new(format!(
            "cannot publish ownership record {} atomically: {error}",
            path.display()
        )));
    }
    fs::remove_file(&temporary)?;
    sync_directory(path.parent())?;
    Ok(path)
}

pub fn update(host_root: &Path, record: &OwnershipRecord) -> ToolResult<PathBuf> {
    ensure_state(host_root)?;
    validate_record_key(&record.resource_key)?;
    let path = record_path(host_root, &record.resource_key);
    ensure_regular_file(&path, "ownership record")?;
    let contents = toml::to_string(record)?;
    let temporary = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("record"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            ToolError::new(format!(
                "cannot create temporary ownership record {}: {error}",
                temporary.display()
            ))
        })?;
    set_private(&file)?;
    let result = file
        .write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            ToolError::new(format!(
                "cannot write ownership record {}: {error}",
                path.display()
            ))
        });
    if result.is_ok() {
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(ToolError::new(format!(
                "cannot publish ownership record {} atomically: {error}",
                path.display()
            )));
        }
        sync_directory(path.parent())?;
    } else {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|_| path)
}

pub fn load(host_root: &Path, resource_key: &str) -> ToolResult<(PathBuf, OwnershipRecord)> {
    validate_record_key(resource_key)?;
    let path = record_path(host_root, resource_key);
    ensure_regular_file(&path, "ownership record").map_err(|error| {
        ToolError::new(format!(
            "no valid ownership record for {resource_key} at {}: {error}; unmanaged resources are never removed",
            path.display()
        ))
    })?;
    let contents = fs::read_to_string(&path).map_err(|error| {
        ToolError::new(format!(
            "ownership record {} cannot be read: {error}; unmanaged resources are never removed",
            path.display()
        ))
    })?;
    let record = toml::from_str::<OwnershipRecord>(&contents).map_err(|error| {
        ToolError::new(format!(
            "ownership record {} is invalid: {error}; repair it manually",
            path.display()
        ))
    })?;
    validate_loaded_record(&record, resource_key).map_err(|error| {
        ToolError::new(format!(
            "ownership record {} is invalid: {error}; repair it manually",
            path.display()
        ))
    })?;
    Ok((path, record))
}

pub fn list(host_root: &Path) -> ToolResult<Vec<RecordEntry>> {
    let directory = records_directory(host_root);
    if !valid_directory(&directory)? {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("toml") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let record = ensure_regular_file(&path, "ownership record")
            .map_err(|error| error.to_string())
            .and_then(|_| fs::read_to_string(&path).map_err(|error| error.to_string()))
            .and_then(|contents| {
                toml::from_str::<OwnershipRecord>(&contents).map_err(|error| error.to_string())
            })
            .and_then(|record| validate_loaded_record(&record, name).map(|_| record));
        entries.push(RecordEntry {
            name: name.to_owned(),
            record,
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

pub fn delete(path: &Path) -> ToolResult<()> {
    ensure_regular_file(path, "ownership record")?;
    fs::remove_file(path).map_err(|error| {
        ToolError::new(format!(
            "owned resource cleanup finished but ownership record {} could not be deleted: {error}; recheck ownership before manual cleanup",
            path.display()
        ))
    })?;
    sync_directory(path.parent())
}

fn validate_loaded_record(record: &OwnershipRecord, key: &str) -> Result<(), String> {
    if record.version != RECORD_VERSION {
        return Err(format!(
            "unsupported record version {}; only version 2 records are accepted",
            record.version
        ));
    }
    validate_record_key(&record.resource_key).map_err(|error| error.to_string())?;
    if record.resource_key != key {
        return Err("record key does not match its filename".to_owned());
    }
    if record.branch != record.workspace_name {
        return Err("branch does not match workspace name".to_owned());
    }
    if record.adapter != "laravel" && record.adapter != "wordpress-component" {
        return Err(format!("unsupported adapter {}", record.adapter));
    }
    Ok(())
}

fn ensure_directory(path: &Path, label: &str) -> ToolResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ToolError::new(format!(
                    "{label} {} is not a regular directory; refusing to use it",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(path)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    Err(ToolError::new(format!(
                        "{label} {} is not a regular directory",
                        path.display()
                    )))
                } else {
                    Ok(())
                }
            }
            Err(error) => Err(ToolError::new(format!(
                "cannot create {label} {}: {error}",
                path.display()
            ))),
        },
        Err(error) => Err(ToolError::new(format!(
            "cannot inspect {label} {}: {error}",
            path.display()
        ))),
    }
}

fn valid_directory(path: &Path) -> ToolResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(true),
        Ok(_) => Err(ToolError::new(format!(
            "ownership directory {} is not a regular directory; refusing to use it",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ToolError::new(format!(
            "cannot inspect ownership directory {}: {error}",
            path.display()
        ))),
    }
}

fn ensure_regular_file(path: &Path, label: &str) -> ToolResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ToolError::new(format!(
            "{label} {} cannot be inspected: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ToolError::new(format!(
            "{label} {} is not a regular file",
            path.display()
        )));
    }
    Ok(())
}

fn validate_record_key(value: &str) -> ToolResult<()> {
    if value.is_empty()
        || value.len() > 63
        || value
            .bytes()
            .any(|byte| !matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'))
        || value.starts_with('-')
        || value.ends_with('-')
    {
        return Err(ToolError::new(format!(
            "resource key {value} is not a safe lowercase name"
        )));
    }
    Ok(())
}

fn validate_lock_name(value: &str) -> ToolResult<()> {
    if value.is_empty()
        || value.len() > 100
        || value
            .bytes()
            .any(|byte| !matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
    {
        return Err(ToolError::new("lock name contains an unsafe character"));
    }
    Ok(())
}

fn sync_directory(path: Option<&Path>) -> ToolResult<()> {
    if let Some(path) = path {
        File::open(path)?.sync_all()?;
    }
    Ok(())
}

fn set_private(file: &File) -> ToolResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn record(host: &Path) -> OwnershipRecord {
        OwnershipRecord {
            version: 2,
            project_id: "fixture".to_owned(),
            workspace_name: "feature/test".to_owned(),
            resource_key: "fixture-feature-test".to_owned(),
            adapter: "laravel".to_owned(),
            repository_root: host.display().to_string(),
            common_directory: host.join(".git").display().to_string(),
            host_root: host.display().to_string(),
            worktree_path: host.join(".worktrees/feature-test").display().to_string(),
            base_sha: "0123456789012345678901234567890123456789".to_owned(),
            branch: "feature/test".to_owned(),
            hostname: "fixture-feature-test.fixture.ddev.site".to_owned(),
            url: "https://fixture-feature-test.fixture.ddev.site".to_owned(),
            database: DatabaseRecord {
                name: "dw_fixture_feature_test".to_owned(),
                provenance: DatabaseProvenance::Copy,
                source: Some("db".to_owned()),
                prepared: false,
                ownership_token: Some("fixture-token".to_owned()),
            },
            paths: OwnedPaths {
                link: host
                    .join(".ddev-workspaces/links/fixture-feature-test")
                    .display()
                    .to_string(),
                view: None,
                files: Vec::new(),
                writable: Vec::new(),
            },
            status: WorkspaceStatus::Creating,
            error: None,
        }
    }

    #[test]
    fn records_are_atomic_and_round_trip() {
        let directory = tempdir().expect("host");
        let expected = record(directory.path());
        let path = reserve(directory.path(), &expected).expect("reserve");
        let (_, loaded) = load(directory.path(), "fixture-feature-test").expect("load");
        assert_eq!(loaded, expected);
        let mut updated = expected.clone();
        updated.status = WorkspaceStatus::Ready;
        update(directory.path(), &updated).expect("atomic update");
        assert_eq!(
            load(directory.path(), "fixture-feature-test").unwrap().1,
            updated
        );
        assert!(path.is_file());
        assert!(
            !fs::read_dir(records_directory(directory.path()))
                .unwrap()
                .any(|entry| entry.unwrap().file_name().to_string_lossy().contains("tmp"))
        );
    }

    #[test]
    fn same_resource_is_exclusive_and_lock_contention_is_refused() {
        let directory = tempdir().expect("host");
        let expected = record(directory.path());
        reserve(directory.path(), &expected).expect("first reservation");
        assert!(reserve(directory.path(), &expected).is_err());
        let first = acquire_workspace_lock(directory.path(), "fixture-feature-test").expect("lock");
        assert!(acquire_workspace_lock(directory.path(), "fixture-feature-test").is_err());
        drop(first);
        assert!(acquire_workspace_lock(directory.path(), "fixture-feature-test").is_ok());
    }

    #[test]
    fn invalid_record_versions_and_keys_are_not_removal_authority() {
        let directory = tempdir().expect("host");
        let mut expected = record(directory.path());
        expected.version = 1;
        reserve(directory.path(), &expected).expect("fixture record");
        assert!(load(directory.path(), "fixture-feature-test").is_err());
        assert!(acquire_workspace_lock(directory.path(), "../unsafe").is_err());
    }

    #[test]
    fn state_ignore_file_must_match_the_owned_contract() {
        let directory = tempdir().expect("host");
        ensure_state(directory.path()).expect("initial state");
        fs::write(state_root(directory.path()).join(".gitignore"), "*.tmp\n").unwrap();
        let error = ensure_state(directory.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match the CLI-owned contract")
        );
    }
}
