use std::fs;
use std::path::{Path, PathBuf};

use crate::command::{CommandRequest, CommandRunner, ToolError, ToolResult};
use crate::config::{self, ProjectConfig, RuntimePaths};
use crate::runtime::{
    PreparedRuntime, ensure_regular_directory, ensure_regular_file, remove_empty_directory,
    remove_owned_file, write_new_file,
};

pub fn prepare(
    config: &ProjectConfig,
    paths: &RuntimePaths,
    workspace: &Path,
    database: &str,
    url: &str,
) -> ToolResult<PreparedRuntime> {
    let mut files = Vec::new();
    let mut writable = Vec::new();
    let result = (|| {
        let docroot = config.runtime.docroot.as_deref().unwrap_or(".");
        let route_target = config::safe_join(workspace, docroot)?;
        ensure_regular_directory(&route_target, "Laravel docroot")?;
        if let Some((source, destination)) = config.environment_source(paths)? {
            let destination = config::safe_join(workspace, &destination.to_string_lossy())?;
            ensure_parent_directories(&destination, workspace, &mut writable)?;
            let contents = fs::read(&source)?;
            let environment = config.environment.as_ref().expect("source exists");
            let replaced = replace_environment(&contents, &environment.replace, database, url)?;
            write_new_file(&destination, replaced.as_bytes(), true)?;
            files.push(destination);
        }
        for carry in &config.carry {
            let source = config::safe_join(&paths.repository_root, &carry.source)?;
            if !source.exists() {
                if carry.required {
                    return Err(crate::command::ToolError::new(format!(
                        "required carry source {} disappeared during preparation",
                        source.display()
                    )));
                }
                continue;
            }
            ensure_regular_file(&source, "carry source")?;
            let destination = carry.destination.as_deref().unwrap_or(&carry.source);
            let destination = config::safe_join(workspace, destination)?;
            ensure_parent_directories(&destination, workspace, &mut writable)?;
            write_new_file(&destination, &fs::read(&source)?, carry.sensitive)?;
            files.push(destination);
        }
        writable.extend(prepare_writable(config, workspace)?);
        let allowed_writable = configured_writable_paths(config, workspace)?;
        Ok(PreparedRuntime {
            route_target,
            view: None,
            files: files.clone(),
            writable: writable.clone(),
            allowed_writable,
        })
    })();
    if result.is_err() {
        for path in files.iter().rev() {
            let _ = remove_owned_file(path);
        }
        for path in writable.iter().rev() {
            let _ = remove_empty_directory(path);
        }
    }
    result
}

pub fn planned_files(
    config: &ProjectConfig,
    paths: &RuntimePaths,
    workspace: &Path,
) -> ToolResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    if let Some((_, destination)) = config.environment_source(paths)? {
        files.push(workspace_join(workspace, &destination.to_string_lossy())?);
    }
    for carry in &config.carry {
        let source = config::safe_join(&paths.repository_root, &carry.source)?;
        if !source.exists() {
            if carry.required {
                return Err(crate::command::ToolError::new(format!(
                    "required carry source {} disappeared during preparation",
                    source.display()
                )));
            }
            continue;
        }
        ensure_regular_file(&source, "carry source")?;
        let destination = carry.destination.as_deref().unwrap_or(&carry.source);
        files.push(workspace_join(workspace, destination)?);
    }
    Ok(files)
}

pub fn configured_file_paths(
    config: &ProjectConfig,
    paths: &RuntimePaths,
    workspace: &Path,
) -> ToolResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    if let Some((_, destination)) = config.environment_source(paths)? {
        files.push(workspace_join(workspace, &destination.to_string_lossy())?);
    }
    for carry in &config.carry {
        let destination = carry.destination.as_deref().unwrap_or(&carry.source);
        files.push(workspace_join(workspace, destination)?);
    }
    Ok(files)
}

pub fn validate_recorded_paths(
    config: &ProjectConfig,
    paths: &RuntimePaths,
    workspace: &Path,
    files: &[PathBuf],
    writable: &[PathBuf],
) -> ToolResult<()> {
    let expected_files = configured_file_paths(config, paths, workspace)?;
    for path in files {
        if !expected_files.iter().any(|expected| expected == path) {
            return Err(ToolError::new(format!(
                "recorded Laravel generated file {} is not declared by the current configuration",
                path.display()
            )));
        }
    }
    let configured_writable = configured_writable_paths(config, workspace)?;
    for path in writable {
        let is_declared_parent = expected_files
            .iter()
            .chain(configured_writable.iter())
            .any(|target| path == target || target.starts_with(path));
        if path == workspace || !is_declared_parent {
            return Err(ToolError::new(format!(
                "recorded Laravel writable path {} is not declared by the current configuration",
                path.display()
            )));
        }
    }
    Ok(())
}

pub fn cleanup<R: CommandRunner>(
    prepared: &PreparedRuntime,
    workspace: &Path,
    runner: &mut R,
) -> ToolResult<()> {
    for path in prepared.files.iter().rev() {
        remove_owned_file(path)?;
    }
    let tracked = tracked_files(workspace, &prepared.allowed_writable, runner)?;
    for path in &prepared.allowed_writable {
        remove_owned_contents(path, workspace, &tracked)?;
    }
    for path in prepared.writable.iter().rev() {
        remove_empty_directory(path)?;
    }
    Ok(())
}

fn tracked_files<R: CommandRunner>(
    workspace: &Path,
    writable: &[PathBuf],
    runner: &mut R,
) -> ToolResult<Vec<PathBuf>> {
    let mut arguments = vec!["ls-files".to_owned(), "-z".to_owned(), "--".to_owned()];
    for path in writable {
        let relative = path.strip_prefix(workspace).map_err(|_| {
            ToolError::new(format!(
                "Laravel writable path {} is outside workspace {}",
                path.display(),
                workspace.display()
            ))
        })?;
        if relative.as_os_str().is_empty() {
            return Err(ToolError::new(
                "Laravel writable path cannot be the workspace root",
            ));
        }
        arguments.push(relative.to_string_lossy().into_owned());
    }
    let output = runner.run(&CommandRequest::new("git", arguments).cwd(workspace))?;
    if !output.success() {
        return Err(ToolError::new(
            "could not inspect tracked Laravel writable files; refusing cleanup",
        ));
    }
    Ok(output
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| workspace.join(path))
        .collect())
}

fn remove_owned_contents(path: &Path, workspace: &Path, tracked: &[PathBuf]) -> ToolResult<()> {
    let relative = path.strip_prefix(workspace).map_err(|_| {
        ToolError::new(format!(
            "Laravel writable path {} is outside workspace {}",
            path.display(),
            workspace.display()
        ))
    })?;
    if relative.as_os_str().is_empty() {
        return Err(ToolError::new(
            "Laravel writable path cannot be the workspace root",
        ));
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(crate::command::ToolError::new(format!(
            "Laravel writable path {} is not a regular directory",
            path.display()
        )));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child)?;
        let is_tracked = tracked.iter().any(|tracked| tracked == &child);
        let has_tracked_descendant = tracked.iter().any(|tracked| {
            tracked
                .strip_prefix(&child)
                .is_ok_and(|relative| !relative.as_os_str().is_empty())
        });
        if is_tracked {
            continue;
        }
        if has_tracked_descendant {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(crate::command::ToolError::new(format!(
                    "Laravel tracked writable path {} has an unsafe ancestor",
                    child.display()
                )));
            }
            remove_owned_contents(&child, workspace, tracked)?;
        } else if metadata.file_type().is_symlink() || metadata.is_file() {
            fs::remove_file(&child)?;
        } else if metadata.is_dir() {
            fs::remove_dir_all(&child)?;
        } else {
            return Err(crate::command::ToolError::new(format!(
                "Laravel writable path {} contains an unsupported filesystem entry",
                child.display()
            )));
        }
    }
    Ok(())
}

fn prepare_writable(config: &ProjectConfig, workspace: &Path) -> ToolResult<Vec<PathBuf>> {
    let defaults = ["storage", "bootstrap/cache"];
    let paths = if config.writable.is_empty() {
        defaults.iter().map(|value| (*value).to_owned()).collect()
    } else {
        config.writable.clone()
    };
    let mut owned = Vec::new();
    for relative in paths {
        let path = config::safe_join(workspace, &relative)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(crate::command::ToolError::new(format!(
                    "Laravel writable path {} is not a regular directory",
                    path.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = ensure_parent_directories(&path, workspace, &mut owned)
                    .and_then(|_| fs::create_dir(&path).map_err(Into::into))
                {
                    for created in owned.iter().rev() {
                        let _ = remove_empty_directory(created);
                    }
                    return Err(error);
                }
                owned.push(path);
            }
            Err(error) => {
                for created in owned.iter().rev() {
                    let _ = remove_empty_directory(created);
                }
                return Err(error.into());
            }
        }
    }
    Ok(owned)
}

pub fn configured_writable_paths(
    config: &ProjectConfig,
    workspace: &Path,
) -> ToolResult<Vec<PathBuf>> {
    let defaults = ["storage", "bootstrap/cache"];
    let paths = if config.writable.is_empty() {
        defaults.iter().map(|value| (*value).to_owned()).collect()
    } else {
        config.writable.clone()
    };
    paths
        .into_iter()
        .map(|relative| workspace_join(workspace, &relative))
        .collect()
}

fn workspace_join(workspace: &Path, relative: &str) -> ToolResult<PathBuf> {
    if workspace.exists() {
        return config::safe_join(workspace, relative);
    }
    config::validate_workspace_relative("generated path", relative)?;
    let candidate = workspace.join(config::normalize_relative_path(relative));
    let workspace_parent = workspace
        .parent()
        .ok_or_else(|| ToolError::new("workspace path has no parent"))?;
    let canonical_parent = fs::canonicalize(workspace_parent).map_err(|error| {
        ToolError::new(format!(
            "cannot resolve workspace parent {}: {error}",
            workspace_parent.display()
        ))
    })?;
    let existing_parent = config::nearest_existing_parent(&candidate);
    let canonical_existing_parent = fs::canonicalize(&existing_parent).map_err(|error| {
        ToolError::new(format!(
            "cannot resolve generated path parent {}: {error}",
            existing_parent.display()
        ))
    })?;
    if !canonical_existing_parent.starts_with(&canonical_parent) {
        return Err(ToolError::new(format!(
            "generated path {} escapes the workspace root",
            candidate.display()
        )));
    }
    Ok(candidate)
}

fn ensure_parent_directories(path: &Path, root: &Path, owned: &mut Vec<PathBuf>) -> ToolResult<()> {
    let Some(mut current) = path.parent() else {
        return Ok(());
    };
    let mut missing = Vec::new();
    while !current.exists() {
        if !current.starts_with(root) {
            return Err(crate::command::ToolError::new(format!(
                "generated path {} escapes the workspace root",
                path.display()
            )));
        }
        missing.push(current.to_path_buf());
        current = current.parent().ok_or_else(|| {
            crate::command::ToolError::new("generated path has no existing workspace parent")
        })?;
    }
    ensure_regular_directory(current, "Laravel generated parent")?;
    for directory in missing.iter().rev() {
        fs::create_dir(directory)?;
        owned.push(directory.clone());
    }
    Ok(())
}

fn replace_environment(
    contents: &[u8],
    keys: &[String],
    database: &str,
    url: &str,
) -> ToolResult<String> {
    let text = String::from_utf8_lossy(contents);
    let mut replaced_keys = Vec::new();
    let mut output = Vec::new();
    for line in text.lines() {
        let Some(key) = environment_assignment_key(line) else {
            output.push(line.to_owned());
            continue;
        };
        if !keys.iter().any(|candidate| candidate == key) {
            output.push(line.to_owned());
            continue;
        }
        if replaced_keys.iter().any(|candidate| *candidate == key) {
            return Err(ToolError::new(format!(
                "Laravel environment source contains duplicate configured key {key}; refusing ambiguous replacement"
            )));
        }
        replaced_keys.push(key.to_owned());
        let leading = &line[..line.len() - line.trim_start().len()];
        let export = line
            .trim_start()
            .strip_prefix("export")
            .is_some_and(|remainder| {
                remainder
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_whitespace())
            });
        let prefix = if export { "export " } else { "" };
        let value = environment_value(key, database, url).ok_or_else(|| {
            ToolError::new(format!(
                "Laravel environment replacement key {key} has no supported workspace value"
            ))
        })?;
        output.push(format!("{leading}{prefix}{key}={value}"));
    }
    for key in keys {
        if !replaced_keys.iter().any(|candidate| candidate == key) {
            return Err(ToolError::new(format!(
                "Laravel environment source is missing configured replacement key {key}"
            )));
        }
    }
    Ok(output.join("\n") + if text.ends_with('\n') { "\n" } else { "" })
}

fn environment_assignment_key(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    let assignment = trimmed
        .strip_prefix("export")
        .filter(|remainder| {
            remainder
                .chars()
                .next()
                .is_some_and(|character| character.is_whitespace())
        })
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let (key, _) = assignment.split_once('=')?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return None;
    }
    Some(key)
}

fn environment_value(key: &str, database: &str, url: &str) -> Option<String> {
    match key {
        "APP_URL" | "WP_HOME" | "WP_SITEURL" => Some(url.to_owned()),
        "DB_DATABASE" | "DB_NAME" => Some(database.to_owned()),
        "DB_HOST" => Some("db".to_owned()),
        "DB_USERNAME" | "DB_USER" => Some("root".to_owned()),
        "DB_PASSWORD" => Some("root".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DatabaseConfig, RuntimeConfig};
    use tempfile::tempdir;

    #[test]
    fn environment_replacement_is_allowlisted() {
        let contents = b"APP_URL=http://main\nDB_DATABASE=main\nOTHER=keep\n";
        let result = replace_environment(
            contents,
            &["APP_URL".to_owned(), "DB_DATABASE".to_owned()],
            "dw_feature",
            "https://feature.example",
        )
        .unwrap();
        assert_eq!(
            result,
            "APP_URL=https://feature.example\nDB_DATABASE=dw_feature\nOTHER=keep\n"
        );
    }

    #[test]
    fn environment_replacement_requires_each_configured_key_once() {
        let missing = replace_environment(
            b"APP_URL=http://main\n",
            &["APP_URL".to_owned(), "DB_DATABASE".to_owned()],
            "dw_feature",
            "https://feature.example",
        )
        .unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("missing configured replacement key DB_DATABASE")
        );

        let exported = replace_environment(
            b"export APP_URL='http://main'\nDB_DATABASE=main\n",
            &["APP_URL".to_owned(), "DB_DATABASE".to_owned()],
            "dw_feature",
            "https://feature.example",
        )
        .unwrap();
        assert_eq!(
            exported,
            "export APP_URL=https://feature.example\nDB_DATABASE=dw_feature\n"
        );

        let duplicate = replace_environment(
            b"APP_URL=http://one\nAPP_URL=http://two\n",
            &["APP_URL".to_owned()],
            "dw_feature",
            "https://feature.example",
        )
        .unwrap_err();
        assert!(
            duplicate
                .to_string()
                .contains("duplicate configured key APP_URL")
        );
    }

    #[test]
    fn preparation_only_creates_declared_environment_and_writable_paths() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("source.env"), "APP_URL=http://main\n").unwrap();
        let config = ProjectConfig {
            version: 2,
            project_id: "fixture".to_owned(),
            worktree_root: ".worktrees".to_owned(),
            runtime: RuntimeConfig {
                adapter: "laravel".to_owned(),
                host: ".".to_owned(),
                docroot: None,
                component_kind: None,
                component_path: None,
            },
            database: DatabaseConfig::default(),
            environment: Some(config::EnvironmentConfig {
                source: "source.env".to_owned(),
                destination: Some(".env".to_owned()),
                replace: vec!["APP_URL".to_owned()],
            }),
            carry: Vec::new(),
            provision: Vec::new(),
            checks: Vec::new(),
            writable: vec!["storage".to_owned(), "bootstrap/cache".to_owned()],
        };
        let paths = RuntimePaths {
            repository_root: directory.path().to_path_buf(),
            host_root: directory.path().to_path_buf(),
            worktree_root: directory.path().join(".worktrees"),
        };
        let mut runner = crate::command::RealCommandRunner::new(false);
        runner
            .run(
                &CommandRequest::new("git", ["init", "--initial-branch=main"])
                    .cwd(directory.path()),
            )
            .unwrap();
        let prepared = prepare(
            &config,
            &paths,
            directory.path(),
            "dw_fixture",
            "https://fixture.example",
        )
        .unwrap();
        assert_eq!(prepared.files.len(), 1);
        assert!(directory.path().join(".env").is_file());
        assert_eq!(
            fs::read_to_string(directory.path().join(".env")).unwrap(),
            "APP_URL=https://fixture.example\n"
        );
        assert_eq!(prepared.writable.len(), 3);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(directory.path().join(".env"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::write(directory.path().join("storage/.gitkeep"), "").unwrap();
        fs::write(directory.path().join("bootstrap/cache/.gitkeep"), "").unwrap();
        runner
            .run(
                &CommandRequest::new(
                    "git",
                    ["add", "--", "storage/.gitkeep", "bootstrap/cache/.gitkeep"],
                )
                .cwd(directory.path()),
            )
            .unwrap();
        fs::create_dir_all(directory.path().join("storage/logs")).unwrap();
        fs::write(directory.path().join("storage/logs/app.log"), "runtime\n").unwrap();
        cleanup(&prepared, directory.path(), &mut runner).unwrap();
        assert!(!directory.path().join(".env").exists());
        assert!(directory.path().join("storage/.gitkeep").exists());
        assert!(directory.path().join("bootstrap/cache/.gitkeep").exists());
        assert!(!directory.path().join("storage/logs").exists());
    }
}
