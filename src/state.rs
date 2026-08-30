use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::command::{ToolError, ToolResult};

pub const STATE_DIRECTORY: &str = "ddev-workspaces/workspaces";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnershipRecord {
    pub version: u32,
    pub project_id: String,
    pub common_directory: String,
    pub worktree_path: String,
    pub base_sha: String,
    pub branch: String,
    pub ddev_name: String,
    pub source_only: bool,
    pub ddev_app_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordEntry {
    pub name: String,
    pub record: Result<OwnershipRecord, String>,
}

pub fn records_directory(common_directory: &Path) -> PathBuf {
    common_directory.join(STATE_DIRECTORY)
}

pub fn record_path(common_directory: &Path, name: &str) -> PathBuf {
    records_directory(common_directory).join(format!("{name}.toml"))
}

pub fn reserve(common_directory: &Path, record: &OwnershipRecord) -> ToolResult<PathBuf> {
    ensure_records_directory(common_directory)?;
    let path = record_path(common_directory, &record.branch);
    let contents = toml::to_string(record)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                ToolError::new(format!(
                    "ownership record already exists at {}; refusing to adopt it",
                    path.display()
                ))
            } else {
                ToolError::new(format!(
                    "cannot reserve ownership record {}: {error}",
                    path.display()
                ))
            }
        })?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
    {
        let _ = fs::remove_file(&path);
        return Err(ToolError::new(format!(
            "initial ownership reservation failed at {}: {error}",
            path.display()
        )));
    }
    Ok(path)
}

pub fn load(common_directory: &Path, name: &str) -> ToolResult<(PathBuf, OwnershipRecord)> {
    let path = record_path(common_directory, name);
    ensure_record_file(&path).map_err(|error| {
        ToolError::new(format!(
            "no valid ownership record for `{name}` at {}: {error}; unmanaged paths are never removed",
            path.display()
        ))
    })?;
    let contents = fs::read_to_string(&path).map_err(|error| {
        ToolError::new(format!(
            "no valid ownership record for `{name}` at {}: {error}; unmanaged paths are never removed",
            path.display()
        ))
    })?;
    let record = toml::from_str::<OwnershipRecord>(&contents).map_err(|error| {
        ToolError::new(format!(
            "ownership record {} is invalid: {error}; repair or remove it manually",
            path.display()
        ))
    })?;
    if record.version != 1 {
        return Err(ToolError::new(format!(
            "ownership record {} has unsupported version {}; v1 refuses it",
            path.display(),
            record.version
        )));
    }
    Ok((path, record))
}

pub fn list(common_directory: &Path) -> ToolResult<Vec<RecordEntry>> {
    let directory = records_directory(common_directory);
    let manager_directory = directory
        .parent()
        .ok_or_else(|| ToolError::new("ownership directory has no parent"))?;
    if !valid_directory(manager_directory)? {
        return Ok(Vec::new());
    }
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
        let record = ensure_record_file(&path)
            .map_err(|error| error.to_string())
            .and_then(|_| fs::read_to_string(&path).map_err(|error| error.to_string()))
            .and_then(|contents| {
                toml::from_str::<OwnershipRecord>(&contents).map_err(|error| error.to_string())
            })
            .and_then(validate_record_version);
        entries.push(RecordEntry {
            name: name.to_owned(),
            record,
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

fn ensure_records_directory(common_directory: &Path) -> ToolResult<()> {
    let directory = records_directory(common_directory);
    let manager_directory = directory
        .parent()
        .ok_or_else(|| ToolError::new("ownership directory has no parent"))?;
    ensure_directory(manager_directory, "ownership manager directory")?;
    ensure_directory(&directory, "ownership directory")
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| {
                ToolError::new(format!("cannot create {label} {}: {error}", path.display()))
            })
        }
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

fn ensure_record_file(path: &Path) -> ToolResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ToolError::new(format!(
            "ownership record {} cannot be inspected: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(ToolError::new(format!(
            "ownership record {} is not a regular file; refusing to use it",
            path.display()
        )));
    }
    Ok(())
}

fn validate_record_version(record: OwnershipRecord) -> Result<OwnershipRecord, String> {
    if record.version == 1 {
        Ok(record)
    } else {
        Err(format!(
            "unsupported ownership record version {}",
            record.version
        ))
    }
}

pub fn delete(path: &Path) -> ToolResult<()> {
    fs::remove_file(path).map_err(|error| {
        ToolError::new(format!(
            "workspace was removed but ownership record {} could not be deleted: {error}; delete it manually only after rechecking ownership",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn record(directory: &Path) -> OwnershipRecord {
        OwnershipRecord {
            version: 1,
            project_id: "fixture".to_owned(),
            common_directory: directory.display().to_string(),
            worktree_path: directory.join(".worktrees/task-1").display().to_string(),
            base_sha: "0123456789012345678901234567890123456789".to_owned(),
            branch: "task-1".to_owned(),
            ddev_name: "dw-fixture--task-1".to_owned(),
            source_only: false,
            ddev_app_root: Some(".".to_owned()),
        }
    }

    #[test]
    fn ownership_record_round_trips_toml_sensitive_values() {
        let directory = tempdir().expect("temporary directory");
        let mut expected = record(directory.path());
        expected.project_id = "quotes \" slashes \\ controls\u{08}\t\n\u{0c}\r\u{1f}".to_owned();
        expected.branch = "unicode-λ".to_owned();

        let encoded = toml::to_string(&expected).expect("serialize ownership record");
        let decoded: OwnershipRecord =
            toml::from_str(&encoded).expect("deserialize ownership record");

        assert_eq!(decoded, expected);
    }

    #[test]
    fn reservation_is_exclusive_and_inspectable() {
        let directory = tempdir().expect("temporary directory");
        let first = record(directory.path());

        let path = reserve(directory.path(), &first).expect("first reservation");
        let second = reserve(directory.path(), &first).unwrap_err();
        let (_, loaded) = load(directory.path(), "task-1").expect("record should load");

        assert!(path.exists());
        assert!(second.to_string().contains("already exists"));
        assert_eq!(loaded, first);
    }

    #[test]
    fn unknown_record_fields_are_rejected() {
        let directory = tempdir().expect("temporary directory");
        let path = record_path(directory.path(), "task-1");
        fs::create_dir_all(path.parent().expect("record parent")).expect("record parent");
        fs::write(
            &path,
            "version = 1\nproject_id = 'fixture'\ncommon_directory = '/tmp'\nworktree_path = '/tmp/task-1'\nbase_sha = '0123456789012345678901234567890123456789'\nbranch = 'task-1'\nddev_name = 'dw-fixture--task-1'\nsource_only = false\nddev_app_root = '.'\nfuture = true\n",
        )
        .expect("record should be written");

        let error = load(directory.path(), "task-1").unwrap_err();

        assert!(error.to_string().contains("invalid"));
    }

    #[test]
    fn listing_is_empty_until_records_exist_then_sorts_and_reports_invalid_records() {
        let directory = tempdir().expect("temporary directory");
        assert!(list(directory.path()).expect("empty listing").is_empty());

        let mut second = record(directory.path());
        second.branch = "second".to_owned();
        reserve(directory.path(), &second).expect("second record");
        let mut first = record(directory.path());
        first.branch = "first".to_owned();
        reserve(directory.path(), &first).expect("first record");
        fs::write(
            records_directory(directory.path()).join("ignored.txt"),
            "ignored",
        )
        .expect("non-record file");
        fs::write(
            records_directory(directory.path()).join("invalid.toml"),
            "version = 2\n",
        )
        .expect("invalid record");

        let entries = list(directory.path()).expect("record listing");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "invalid", "second"]
        );
        assert!(entries[0].record.is_ok());
        assert!(entries[1].record.is_err());
        assert!(entries[2].record.is_ok());
    }

    #[test]
    fn missing_and_unsupported_records_are_never_loaded_or_deleted_as_owned() {
        let directory = tempdir().expect("temporary directory");
        assert!(load(directory.path(), "missing").is_err());

        let mut unsupported = record(directory.path());
        unsupported.version = 2;
        let path = reserve(directory.path(), &unsupported).expect("unsupported record fixture");
        assert!(
            load(directory.path(), "task-1")
                .unwrap_err()
                .to_string()
                .contains("unsupported version 2")
        );

        delete(&path).expect("record deletion");
        assert!(!path.exists());
        assert!(delete(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_ownership_directories_and_records_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("temporary directory");
        let outside = tempdir().expect("outside directory");
        fs::create_dir(directory.path().join("ddev-workspaces"))
            .expect("ownership manager directory");
        symlink(
            outside.path(),
            directory.path().join("ddev-workspaces/workspaces"),
        )
        .expect("ownership directory symlink");

        assert!(reserve(directory.path(), &record(directory.path())).is_err());
        assert!(list(directory.path()).is_err());

        fs::remove_file(directory.path().join("ddev-workspaces/workspaces"))
            .expect("remove ownership symlink");
        fs::create_dir(directory.path().join("ddev-workspaces/workspaces"))
            .expect("ownership directory");
        let target = outside.path().join("record.toml");
        fs::write(&target, "version = 1\n").expect("record target");
        symlink(&target, record_path(directory.path(), "task-1")).expect("record symlink");
        assert!(load(directory.path(), "task-1").is_err());
    }
}
