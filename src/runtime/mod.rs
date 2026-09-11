pub mod database;
pub mod host;
pub mod laravel;
pub mod wordpress;

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;
use signal_hook::low_level;

use crate::command::{ToolError, ToolResult};
use crate::ddev;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRuntime {
    pub route_target: PathBuf,
    pub view: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    pub writable: Vec<PathBuf>,
    pub allowed_writable: Vec<PathBuf>,
}

pub struct SignalGuard {
    interrupted: Arc<AtomicBool>,
    handlers: [signal_hook::SigId; 4],
}

impl SignalGuard {
    pub fn install() -> ToolResult<Self> {
        let interrupted = Arc::new(AtomicBool::new(false));
        let sigint_shutdown =
            flag::register_conditional_shutdown(SIGINT, 130, Arc::clone(&interrupted))?;
        let sigint_flag = match flag::register(SIGINT, Arc::clone(&interrupted)) {
            Ok(id) => id,
            Err(error) => {
                low_level::unregister(sigint_shutdown);
                return Err(error.into());
            }
        };
        let sigterm_shutdown =
            match flag::register_conditional_shutdown(SIGTERM, 143, Arc::clone(&interrupted)) {
                Ok(id) => id,
                Err(error) => {
                    low_level::unregister(sigint_shutdown);
                    low_level::unregister(sigint_flag);
                    return Err(error.into());
                }
            };
        let sigterm_flag = match flag::register(SIGTERM, Arc::clone(&interrupted)) {
            Ok(id) => id,
            Err(error) => {
                low_level::unregister(sigint_shutdown);
                low_level::unregister(sigint_flag);
                low_level::unregister(sigterm_shutdown);
                return Err(error.into());
            }
        };
        Ok(Self {
            interrupted,
            handlers: [sigint_shutdown, sigint_flag, sigterm_shutdown, sigterm_flag],
        })
    }

    pub fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Relaxed)
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        for handler in self.handlers {
            low_level::unregister(handler);
        }
    }
}

struct TemporaryPathGuard {
    path: PathBuf,
}

impl Drop for TemporaryPathGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn ensure_regular_directory(path: &Path, label: &str) -> ToolResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ToolError::new(format!(
                    "{label} {} is not a regular directory",
                    path.display()
                )));
            }
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|error| {
                ToolError::new(format!("cannot create {label} {}: {error}", path.display()))
            })?;
            Ok(true)
        }
        Err(error) => Err(ToolError::new(format!(
            "cannot inspect {label} {}: {error}",
            path.display()
        ))),
    }
}

pub fn ensure_regular_file(path: &Path, label: &str) -> ToolResult<()> {
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

pub fn write_new_file(path: &Path, contents: &[u8], private: bool) -> ToolResult<()> {
    if let Some(parent) = path.parent() {
        ensure_regular_directory(parent, "file parent")?;
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(ToolError::new(format!(
                "generated file {} already exists; refusing to overwrite an unproven file",
                path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let temporary = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        std::process::id()
    ));
    let _temporary_guard = TemporaryPathGuard {
        path: temporary.clone(),
    };
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(contents)?;
    if private {
        set_private(&file)?;
    }
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

pub fn remove_owned_file(path: &Path) -> ToolResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
            fs::remove_file(path)?;
            Ok(())
        }
        Ok(_) => Err(ToolError::new(format!(
            "owned generated path {} is not a removable file",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn remove_empty_directory(path: &Path) -> ToolResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ToolError::new(format!(
            "owned writable path {} became a symlink; refusing removal",
            path.display()
        ))),
        Ok(metadata) if metadata.is_dir() => match fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(()),
            Err(error) => Err(error.into()),
        },
        Ok(_) => Err(ToolError::new(format!(
            "owned writable path {} is not a directory",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn set_private(file: &fs::File) -> ToolResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)?;
    }
    Ok(())
}

pub fn create_symlink(target: &Path, link: &Path) -> ToolResult<()> {
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
            "workspace views require a platform with symbolic links",
        ))
    }
}

pub fn symlink_or_error(target: &Path, link: &Path, allowed_root: &Path) -> ToolResult<()> {
    ensure_tree_within(target, allowed_root)?;
    if let Some(parent) = link.parent() {
        ensure_regular_directory(parent, "symlink parent")?;
    }
    match fs::symlink_metadata(link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let actual = fs::canonicalize(link)?;
            let expected = fs::canonicalize(target)?;
            let root = fs::canonicalize(allowed_root)?;
            if !actual.starts_with(&root) || !expected.starts_with(&root) {
                return Err(ToolError::new(format!(
                    "generated symlink {} escapes authorized root {}",
                    link.display(),
                    root.display()
                )));
            }
            if actual == expected {
                Ok(())
            } else {
                Err(ToolError::new(format!(
                    "generated symlink {} points to {}, not {}",
                    link.display(),
                    actual.display(),
                    expected.display()
                )))
            }
        }
        Ok(_) => Err(ToolError::new(format!(
            "generated view path {} already exists and is not a symlink",
            link.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let relative_target = ddev::relative_path(
                link.parent()
                    .ok_or_else(|| ToolError::new("generated symlink has no parent"))?,
                target,
            )?;
            create_symlink(&relative_target, link)
        }
        Err(error) => Err(error.into()),
    }
}

pub fn ensure_tree_within(path: &Path, allowed_root: &Path) -> ToolResult<()> {
    let root = fs::canonicalize(allowed_root).map_err(|error| {
        ToolError::new(format!(
            "authorized link root {} cannot be resolved: {error}",
            allowed_root.display()
        ))
    })?;
    let mut visited = HashSet::new();
    ensure_tree_within_inner(path, &root, &mut visited)
}

fn ensure_tree_within_inner(
    path: &Path,
    root: &Path,
    visited: &mut HashSet<PathBuf>,
) -> ToolResult<()> {
    let _metadata = fs::symlink_metadata(path).map_err(|error| {
        ToolError::new(format!(
            "link source {} cannot be inspected: {error}",
            path.display()
        ))
    })?;
    let resolved = fs::canonicalize(path).map_err(|error| {
        ToolError::new(format!(
            "link source {} cannot be resolved: {error}",
            path.display()
        ))
    })?;
    if !resolved.starts_with(root) {
        return Err(ToolError::new(format!(
            "link source {} resolves outside authorized root {}",
            path.display(),
            root.display()
        )));
    }
    if !fs::metadata(&resolved)?.is_dir() {
        return Ok(());
    }
    if !visited.insert(resolved) {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        ensure_tree_within_inner(&entry?.path(), root, visited)?;
    }
    Ok(())
}
