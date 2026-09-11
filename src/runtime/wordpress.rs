use std::fs;
use std::path::{Path, PathBuf};

use crate::command::{ToolError, ToolResult};
use crate::config::{self, ProjectConfig, RuntimePaths};
use crate::runtime::{
    PreparedRuntime, ensure_regular_directory, ensure_regular_file, remove_owned_file,
    symlink_or_error, write_new_file,
};
use crate::state;

const ROOT_DIRECTORIES: &[&str] = &["vendor", "wp-admin", "wp-includes"];
const ROOT_FILES: &[&str] = &[
    "ads.txt",
    "favicon.ico",
    "index.php",
    "license.txt",
    "readme.html",
    "robots.txt",
    "sitemap.xml",
    "sitemap_index.xml",
    "wp-activate.php",
    "wp-blog-header.php",
    "wp-comments-post.php",
    "wp-cron.php",
    "wp-links-opml.php",
    "wp-load.php",
    "wp-login.php",
    "wp-mail.php",
    "wp-settings.php",
    "wp-signup.php",
    "wp-trackback.php",
    "xmlrpc.php",
];
const WRITABLE_CONTENT: &[&str] = &["uploads", "cache", "upgrade", "languages", "mu-cache"];
const IMMUTABLE_CONTENT_DIRECTORIES: &[&str] = &["mu-plugins"];
const IMMUTABLE_CONTENT_FILES: &[&str] = &["advanced-cache.php", "index.php", "object-cache.php"];
const VIEW_OWNERSHIP_MARKER: &str = ".ddev-workspaces-owned";

pub fn prepare(
    config: &ProjectConfig,
    paths: &RuntimePaths,
    workspace: &Path,
    resource_key: &str,
    database: &str,
    url: &str,
) -> ToolResult<PreparedRuntime> {
    let component_relative = selected_component_relative(config)?;
    let component_parent = configured_component_parent(config)?;
    let selected_relative = component_relative
        .strip_prefix(&component_parent)
        .map_err(|_| ToolError::new("WordPress component path is outside its configured kind"))?
        .to_path_buf();
    let view = state::views_directory(&paths.host_root).join(resource_key);
    ensure_view_directory(&view)?;
    let marker = view.join(VIEW_OWNERSHIP_MARKER);
    if let Err(error) = write_new_file(&marker, format!("{resource_key}\n").as_bytes(), true) {
        match fs::remove_dir(&view) {
            Ok(()) => {}
            Err(cleanup_error) if cleanup_error.kind() == std::io::ErrorKind::NotFound => {}
            Err(cleanup_error) => {
                return Err(ToolError::new(format!(
                    "{error}; cannot remove the incomplete WordPress generated view {}: {cleanup_error}",
                    view.display()
                )));
            }
        }
        return Err(error);
    }
    let result = (|| {
        for entry in fs::read_dir(paths.host_root.join("."))? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "wp-config.php"
                || entry.path().starts_with(&paths.worktree_root)
                || (!ROOT_DIRECTORIES.iter().any(|allowed| *allowed == name)
                    && !ROOT_FILES.iter().any(|allowed| *allowed == name))
            {
                continue;
            }
            ensure_shared_source_is_safe(&entry.path(), &paths.host_root)?;
            symlink_or_error(&entry.path(), &view.join(name.as_ref()), &paths.host_root)?;
        }
        ensure_regular_directory(&view.join("wp-content"), "WordPress generated wp-content")?;
        for entry in fs::read_dir(paths.host_root.join("wp-content"))? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "plugins" || name == "themes" {
                let destination = view.join("wp-content").join(name.as_ref());
                ensure_regular_directory(&destination, "WordPress component directory")?;
                let selected = if name == component_parent.file_name().unwrap().to_string_lossy() {
                    selected_relative.clone()
                } else {
                    PathBuf::from("__component_not_here__")
                };
                link_tree_except(
                    &entry.path(),
                    &destination,
                    &selected,
                    workspace,
                    &paths.host_root,
                )?;
            } else if WRITABLE_CONTENT.iter().any(|writable| *writable == name) {
                ensure_regular_directory(
                    &view.join("wp-content").join(name.as_ref()),
                    "WordPress writable path",
                )?;
            } else if IMMUTABLE_CONTENT_DIRECTORIES
                .iter()
                .any(|immutable| *immutable == name)
                || IMMUTABLE_CONTENT_FILES
                    .iter()
                    .any(|immutable| *immutable == name)
            {
                ensure_shared_source_is_safe(&entry.path(), &paths.host_root)?;
                symlink_or_error(
                    &entry.path(),
                    &view.join("wp-content").join(name.as_ref()),
                    &paths.host_root,
                )?;
            } else {
                return Err(ToolError::new(format!(
                    "unsupported WordPress wp-content entry `{name}`; declare it as writable or add an immutable adapter rule"
                )));
            }
        }
        let selected_destination = view.join(&component_relative).to_path_buf();
        if !selected_destination.is_symlink() {
            return Err(ToolError::new(
                "WordPress generated view did not publish the selected component",
            ));
        }
        let config_source = paths.host_root.join("wp-config.php");
        ensure_regular_file(&config_source, "WordPress host configuration")?;
        let config_destination = view.join("wp-config.php");
        let generated = render_wp_config(&fs::read_to_string(&config_source)?, database, url)?;
        write_new_file(&config_destination, generated.as_bytes(), true)?;
        let writable = writable_paths(&view)?;
        Ok(PreparedRuntime {
            route_target: view.clone(),
            view: Some(view.clone()),
            files: vec![config_destination, marker],
            allowed_writable: writable.clone(),
            writable,
        })
    })();
    if result.is_err() && matches!(view_is_owned(&view, resource_key), Ok(true)) {
        let _ = fs::remove_dir_all(&view);
    }
    result
}

pub fn view_is_owned(view: &Path, resource_key: &str) -> ToolResult<bool> {
    let metadata = match fs::symlink_metadata(view) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(false);
    }
    let marker = view.join(VIEW_OWNERSHIP_MARKER);
    let metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(false);
    }
    Ok(fs::read_to_string(marker)? == format!("{resource_key}\n"))
}

pub fn view_is_complete(
    config: &ProjectConfig,
    view: &Path,
    workspace: &Path,
    resource_key: &str,
    files: &[PathBuf],
    writable: &[PathBuf],
) -> ToolResult<bool> {
    if !view_is_owned(view, resource_key)? || !selected_component_is_owned(config, view, workspace)?
    {
        return Ok(false);
    }
    let config_path = view.join("wp-config.php");
    match fs::symlink_metadata(&config_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) | Err(_) => return Ok(false),
    }
    let expected_files = [config_path, view.join(VIEW_OWNERSHIP_MARKER)];
    if files.len() != expected_files.len()
        || expected_files
            .iter()
            .any(|expected| !files.iter().any(|actual| actual == expected))
    {
        return Ok(false);
    }
    Ok(writable_paths(view)? == writable)
}

pub fn remove_owned_view(view: &Path, resource_key: &str) -> ToolResult<()> {
    if !view_is_owned(view, resource_key)? {
        return Err(ToolError::new(format!(
            "WordPress generated view {} is not proven workspace-owned; refusing recovery removal",
            view.display()
        )));
    }
    fs::remove_dir_all(view).map_err(|error| {
        ToolError::new(format!(
            "cannot remove incomplete owned WordPress generated view {}: {error}",
            view.display()
        ))
    })
}

pub fn selected_component_is_owned(
    config: &ProjectConfig,
    view: &Path,
    workspace: &Path,
) -> ToolResult<bool> {
    let component = view.join(selected_component_relative(config)?);
    let metadata = match fs::symlink_metadata(&component) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let actual = match fs::canonicalize(&component) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let expected = match fs::canonicalize(workspace) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(actual == expected)
}

pub fn validate_recorded_paths(
    view: &Path,
    files: &[PathBuf],
    writable: &[PathBuf],
) -> ToolResult<()> {
    let expected_files = [view.join("wp-config.php"), view.join(VIEW_OWNERSHIP_MARKER)];
    for path in files {
        if !expected_files.iter().any(|expected| expected == path) {
            return Err(ToolError::new(format!(
                "recorded WordPress generated file {} is not owned by the generated view",
                path.display()
            )));
        }
    }
    let expected_writable = WRITABLE_CONTENT
        .iter()
        .map(|name| view.join("wp-content").join(name))
        .collect::<Vec<_>>();
    for path in writable {
        if !expected_writable.iter().any(|expected| expected == path) {
            return Err(ToolError::new(format!(
                "recorded WordPress writable path {} is not owned by the generated view",
                path.display()
            )));
        }
    }
    Ok(())
}

pub fn ensure_selected_component_is_safe(
    config: &ProjectConfig,
    view: &Path,
    workspace: &Path,
) -> ToolResult<()> {
    let component = view.join(selected_component_relative(config)?);
    crate::runtime::ensure_tree_within(&component, workspace)
}

pub fn cleanup(prepared: &PreparedRuntime) -> ToolResult<()> {
    let Some(view) = &prepared.view else {
        for path in prepared.files.iter().rev() {
            remove_owned_file(path)?;
        }
        return Ok(());
    };
    let metadata = match fs::symlink_metadata(view) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ToolError::new(format!(
                "WordPress generated view {} cannot be inspected: {error}",
                view.display()
            )));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ToolError::new(format!(
            "WordPress generated view {} is not a regular directory",
            view.display()
        )));
    }
    if !view_has_ownership_marker(view)? {
        return Err(ToolError::new(format!(
            "WordPress generated view {} is not proven workspace-owned; inspect it manually before removal",
            view.display()
        )));
    }
    for path in prepared.files.iter().rev().filter(|path| {
        path.file_name()
            .is_none_or(|name| name != std::ffi::OsStr::new(VIEW_OWNERSHIP_MARKER))
    }) {
        remove_owned_file(path)?;
    }
    fs::remove_dir_all(view).map_err(|error| {
        ToolError::new(format!(
            "cannot remove owned WordPress generated view {}: {error}",
            view.display()
        ))
    })
}

fn view_has_ownership_marker(view: &Path) -> ToolResult<bool> {
    match fs::symlink_metadata(view.join(VIEW_OWNERSHIP_MARKER)) {
        Ok(metadata) => Ok(metadata.is_file() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn ensure_view_directory(view: &Path) -> ToolResult<()> {
    match fs::symlink_metadata(view) {
        Ok(_) => Err(ToolError::new(format!(
            "WordPress generated view {} already exists; refusing to adopt it",
            view.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = view.parent() {
                ensure_regular_directory(parent, "WordPress view parent")?;
            }
            fs::create_dir(view)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn link_tree_except(
    source: &Path,
    destination: &Path,
    selected: &Path,
    workspace: &Path,
    host_root: &Path,
) -> ToolResult<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let relative = PathBuf::from(entry.file_name());
        if relative.to_string_lossy().starts_with('.') {
            continue;
        }
        if selected.starts_with(&relative) {
            let destination_entry = destination.join(&relative);
            if relative == selected {
                symlink_or_error(workspace, &destination_entry, workspace)?;
            } else {
                ensure_shared_source_is_safe(&entry.path(), host_root)?;
                ensure_regular_directory(&destination_entry, "WordPress component ancestor")?;
                link_tree_except(
                    &entry.path(),
                    &destination_entry,
                    selected
                        .strip_prefix(&relative)
                        .unwrap_or_else(|_| Path::new("")),
                    workspace,
                    host_root,
                )?;
            }
        } else {
            ensure_shared_source_is_safe(&entry.path(), host_root)?;
            symlink_or_error(&entry.path(), &destination.join(relative), host_root)?;
        }
    }
    Ok(())
}

fn ensure_shared_source_is_safe(source: &Path, host_root: &Path) -> ToolResult<()> {
    let resolved = fs::canonicalize(source).map_err(|error| {
        ToolError::new(format!(
            "WordPress shared source {} cannot be resolved: {error}",
            source.display()
        ))
    })?;
    let canonical_host_root = fs::canonicalize(host_root)?;
    if resolved == canonical_host_root {
        return Err(ToolError::new(
            "WordPress shared source resolves to the host root; refusing recursive exposure",
        ));
    }
    if resolved.components().any(|component| {
        matches!(
            component,
            std::path::Component::Normal(name)
                if matches!(name.to_str(), Some(".ddev" | ".ddev-workspaces" | ".git"))
        )
    }) {
        return Err(ToolError::new(format!(
            "WordPress shared source {} resolves into protected metadata; refusing exposure",
            source.display()
        )));
    }
    for forbidden in [".ddev", ".ddev-workspaces", ".git"] {
        let path = host_root.join(forbidden);
        let Ok(canonical_forbidden) = fs::canonicalize(&path) else {
            continue;
        };
        if resolved == canonical_forbidden || resolved.starts_with(&canonical_forbidden) {
            return Err(ToolError::new(format!(
                "WordPress shared source {} resolves into protected host state {}; refusing exposure",
                source.display(),
                path.display()
            )));
        }
    }
    Ok(())
}

fn writable_paths(view: &Path) -> ToolResult<Vec<PathBuf>> {
    let content = view.join("wp-content");
    let mut paths = Vec::new();
    for name in WRITABLE_CONTENT {
        let path = content.join(name);
        if path.is_dir() && !path.is_symlink() {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn render_wp_config(source: &str, database: &str, url: &str) -> ToolResult<String> {
    let values = [
        ("DB_NAME", database),
        ("DB_USER", "root"),
        ("DB_PASSWORD", "root"),
        ("DB_HOST", "db"),
        ("WP_HOME", url),
        ("WP_SITEURL", url),
    ];
    for (key, _) in values {
        if has_multiline_define(source, key) {
            return Err(ToolError::new(format!(
                "WordPress configuration contains a multiline define for {key}; rewrite it as a single-line literal before creating a workspace"
            )));
        }
        if has_const_definition(source, key) {
            return Err(ToolError::new(format!(
                "WordPress configuration contains a const definition for {key}; rewrite it as a single-line define before creating a workspace"
            )));
        }
    }
    for line in source.lines() {
        for (key, _) in values {
            if contains_define_for_key(line, key) && !defines(line, key) {
                return Err(ToolError::new(format!(
                    "WordPress configuration contains a non-standalone define for {key}; rewrite it as a single-line define before creating a workspace"
                )));
            }
        }
    }
    let mut output = String::new();
    let mut found = [false; 6];
    for line in source.lines() {
        let mut replaced = false;
        for (index, (key, value)) in values.iter().enumerate() {
            if defines(line, key) {
                output.push_str(&format!("define('{}', '{}');", key, php_escape(value)));
                output.push('\n');
                found[index] = true;
                replaced = true;
                break;
            }
        }
        if !replaced {
            output.push_str(line);
            output.push('\n');
        }
    }
    let insert_at = wp_config_insert_offset(&output);
    let missing = values
        .iter()
        .enumerate()
        .filter(|(index, _)| !found[*index])
        .map(|(_, (key, value))| format!("define('{}', '{}');\n", key, php_escape(value)))
        .collect::<String>();
    output.insert_str(insert_at, &missing);
    Ok(output)
}

fn wp_config_insert_offset(source: &str) -> usize {
    source
        .find("require_once")
        .or_else(|| source.find("require "))
        .or_else(|| source.find("include_once"))
        .or_else(|| {
            let open = source.find("<?php")?;
            source[open..]
                .find('\n')
                .map(|offset| open + offset + 1)
                .or(Some(source.len()))
        })
        .unwrap_or(0)
}

fn selected_component_relative(config: &ProjectConfig) -> ToolResult<PathBuf> {
    let component_path = config
        .runtime
        .component_path
        .as_deref()
        .ok_or_else(|| ToolError::new("WordPress component path is missing"))?;
    config::validate_repository_relative("runtime.component_path", component_path)?;
    let component_relative = config::normalize_relative_path(component_path);
    let component_parent = configured_component_parent(config)?;
    if !component_relative.starts_with(&component_parent) || component_relative == component_parent
    {
        return Err(ToolError::new(
            "WordPress component path is outside its configured kind",
        ));
    }
    Ok(component_relative)
}

fn configured_component_parent(config: &ProjectConfig) -> ToolResult<PathBuf> {
    match config.runtime.component_kind.as_deref() {
        Some("plugin") => Ok(Path::new("wp-content").join("plugins")),
        Some("theme") => Ok(Path::new("wp-content").join("themes")),
        Some(_) => Err(ToolError::new(
            "WordPress component kind must be plugin or theme",
        )),
        None => Err(ToolError::new("WordPress component kind is missing")),
    }
}

fn has_multiline_define(source: &str, key: &str) -> bool {
    let mut offset = 0;
    while let Some(relative_start) = source[offset..].find("define") {
        let start = offset + relative_start;
        let after_name = &source[start + "define".len()..];
        let whitespace = after_name
            .char_indices()
            .find(|(_, character)| !character.is_whitespace())
            .map(|(index, _)| index);
        let Some(open_offset) = whitespace else {
            break;
        };
        if !after_name[open_offset..].starts_with('(') {
            offset = start + "define".len();
            continue;
        }
        let call_start = start;
        let mut depth = 0usize;
        let mut call_end = source.len();
        for (index, character) in source[call_start..].char_indices() {
            match character {
                '(' => depth += 1,
                ')' if depth > 0 => {
                    depth -= 1;
                    if depth == 0 {
                        call_end = call_start + index + character.len_utf8();
                        break;
                    }
                }
                _ => {}
            }
        }
        let call = &source[call_start..call_end];
        if call.contains('\n')
            && (call.contains(&format!("'{key}'")) || call.contains(&format!("\"{key}\"")))
        {
            return true;
        }
        offset = call_end.max(start + "define".len());
    }
    false
}

fn defines(line: &str, key: &str) -> bool {
    let trimmed = line.trim();
    let Some(arguments) = trimmed.strip_prefix("define") else {
        return false;
    };
    let arguments = arguments.trim_start();
    let Some(arguments) = arguments.strip_prefix('(') else {
        return false;
    };
    let Some(close) = arguments.find(')') else {
        return false;
    };
    if arguments[close + 1..].trim() != ";" {
        return false;
    }
    let first_argument = arguments[..close]
        .split_once(',')
        .map_or_else(|| arguments[..close].trim(), |(first, _)| first.trim());
    first_argument
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .or_else(|| {
            first_argument
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
        == Some(key)
}

fn contains_define_for_key(line: &str, key: &str) -> bool {
    line.contains("define")
        && (line.contains(&format!("'{key}'")) || line.contains(&format!("\"{key}\"")))
}

fn has_const_definition(source: &str, key: &str) -> bool {
    source.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.strip_prefix("const").is_some_and(|rest| {
            rest.chars()
                .next()
                .is_some_and(|character| character.is_whitespace())
                && rest
                    .split_once('=')
                    .is_some_and(|(name, _)| name.trim() == key)
        })
    })
}

fn php_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn generated_config_contains_workspace_database_and_url_without_source_secret_values() {
        let source = "<?php\ndefine(\"DB_NAME\", \"main\");\ndefine('DB_USER', 'old');\nrequire_once ABSPATH . 'wp-settings.php';\n";
        let generated = render_wp_config(source, "dw_feature", "https://feature.example").unwrap();
        assert!(generated.contains("define('DB_NAME', 'dw_feature');"));
        assert!(generated.contains("define('DB_HOST', 'db');"));
        assert!(generated.contains("https://feature.example"));
        assert!(!generated.contains("'main'"));
    }

    #[test]
    fn multiline_managed_defines_are_rejected_before_rendering() {
        let source = "<?php\ndefine(\n    'DB_NAME',\n    'source_db'\n);\n";
        let error = render_wp_config(source, "dw_feature", "https://feature.example").unwrap_err();
        assert!(error.to_string().contains("multiline define for DB_NAME"));
    }

    #[test]
    fn complex_managed_define_lines_are_rejected_before_rendering() {
        for source in [
            "<?php\nif ($use_alt) define('DB_NAME', 'source_db');\n",
            "<?php\n// define('DB_NAME', 'source_db');\n",
            "<?php\ndefine('DB_NAME', 'source_db'); bootstrap();\n",
        ] {
            let error =
                render_wp_config(source, "dw_feature", "https://feature.example").unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("non-standalone define for DB_NAME")
            );
        }
    }

    fn wordpress_config(component_path: &str) -> ProjectConfig {
        ProjectConfig {
            version: 2,
            project_id: "fixture".to_owned(),
            worktree_root: ".ddev-workspaces/worktrees".to_owned(),
            runtime: config::RuntimeConfig {
                adapter: "wordpress-component".to_owned(),
                host: ".".to_owned(),
                docroot: None,
                component_kind: Some("plugin".to_owned()),
                component_path: Some(component_path.to_owned()),
            },
            database: config::DatabaseConfig::default(),
            environment: None,
            carry: Vec::new(),
            provision: Vec::new(),
            checks: Vec::new(),
            writable: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn partial_owned_views_are_not_treated_as_complete() {
        let directory = tempdir().unwrap();
        let host = directory.path().join("host");
        let workspace = host.join(".ddev-workspaces/worktrees/partial");
        let view = host.join(".ddev-workspaces/views/fixture-partial");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&view).unwrap();
        fs::write(view.join(VIEW_OWNERSHIP_MARKER), "fixture-partial\n").unwrap();
        let config = wordpress_config("wp-content/plugins/example");

        assert!(
            !view_is_complete(&config, &view, &workspace, "fixture-partial", &[], &[],).unwrap()
        );
        remove_owned_view(&view, "fixture-partial").unwrap();
        assert!(fs::symlink_metadata(view).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn component_views_isolate_selected_worktrees_and_cleanup_owned_state() {
        let directory = tempdir().unwrap();
        let host = directory.path().join("host");
        let workspace_one = host.join(".ddev-workspaces/worktrees/one");
        let workspace_two = host.join(".ddev-workspaces/worktrees/two");
        fs::create_dir_all(host.join("wp-content/plugins/example")).unwrap();
        fs::create_dir_all(host.join("wp-content/plugins/other")).unwrap();
        fs::create_dir_all(host.join("wp-content/themes/default")).unwrap();
        fs::create_dir_all(host.join("wp-admin")).unwrap();
        fs::create_dir_all(host.join("wp-includes")).unwrap();
        fs::create_dir_all(host.join("wp-content/uploads")).unwrap();
        fs::create_dir_all(host.join("wp-content/cache")).unwrap();
        fs::create_dir_all(host.join("wp-content/mu-plugins")).unwrap();
        fs::write(host.join("wp-content/index.php"), "<?php\n").unwrap();
        fs::write(host.join("wp-admin/index.php"), "<?php\n").unwrap();
        fs::write(host.join("wp-includes/version.php"), "<?php\n").unwrap();
        fs::write(host.join("index.php"), "<?php\n").unwrap();
        fs::write(host.join("auth.json"), "{\"token\":\"parent-secret\"}\n").unwrap();
        fs::write(
            host.join("wp-config.php"),
            "<?php\ndefine(\"DB_NAME\", \"source_db\");\ndefine(\"DB_USER\", \"source_user\");\ndefine(\"DB_PASSWORD\", \"source_secret\");\ndefine(\"DB_HOST\", \"source_host\");\ndefine(\"WP_HOME\", \"https://source.example\");\ndefine(\"WP_SITEURL\", \"https://source.example\");\n",
        )
        .unwrap();
        fs::create_dir_all(&workspace_one).unwrap();
        fs::create_dir_all(&workspace_two).unwrap();
        fs::write(workspace_one.join("plugin.php"), "<?php\n").unwrap();
        fs::write(workspace_two.join("plugin.php"), "<?php\n").unwrap();
        state::ensure_state(&host).unwrap();
        let config = wordpress_config("wp-content/plugins/example");
        let paths = RuntimePaths {
            repository_root: workspace_one.clone(),
            host_root: host.clone(),
            worktree_root: host.join(".ddev-workspaces/worktrees"),
        };

        let first = prepare(
            &config,
            &paths,
            &workspace_one,
            "fixture-one",
            "dw_one",
            "https://one.example",
        )
        .unwrap();
        let second = prepare(
            &config,
            &paths,
            &workspace_two,
            "fixture-two",
            "dw_two",
            "https://two.example",
        )
        .unwrap();

        assert_eq!(
            fs::canonicalize(
                host.join(".ddev-workspaces/views/fixture-one/wp-content/plugins/example")
            )
            .unwrap(),
            fs::canonicalize(&workspace_one).unwrap()
        );
        assert_eq!(
            fs::canonicalize(
                host.join(".ddev-workspaces/views/fixture-two/wp-content/plugins/example")
            )
            .unwrap(),
            fs::canonicalize(&workspace_two).unwrap()
        );
        assert!(
            !host
                .join(".ddev-workspaces/views/fixture-one/wp-content/uploads")
                .is_symlink()
        );
        assert!(
            !host
                .join(".ddev-workspaces/views/fixture-two/wp-content/uploads")
                .is_symlink()
        );
        let first_config = fs::read_to_string(&first.files[0]).unwrap();
        let second_config = fs::read_to_string(&second.files[0]).unwrap();
        assert!(first_config.contains("define('DB_NAME', 'dw_one');"));
        assert!(second_config.contains("define('DB_NAME', 'dw_two');"));
        assert!(first_config.contains("https://one.example"));
        assert!(!first_config.contains("source_secret"));
        assert!(
            !host
                .join(".ddev-workspaces/views/fixture-one/auth.json")
                .exists()
        );

        cleanup(&first).unwrap();
        assert!(!host.join(".ddev-workspaces/views/fixture-one").exists());
        assert!(host.join(".ddev-workspaces/views/fixture-two").exists());
        cleanup(&second).unwrap();
        assert!(!host.join(".ddev-workspaces/views/fixture-two").exists());
        fs::create_dir(host.join("wp-content/custom-cache")).unwrap();
        let error = prepare(
            &config,
            &paths,
            &workspace_one,
            "fixture-three",
            "dw_three",
            "https://three.example",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported WordPress wp-content entry")
        );
    }

    #[cfg(unix)]
    #[test]
    fn component_views_reject_host_entries_that_escape_the_host_root() {
        let directory = tempdir().unwrap();
        let host = directory.path().join("host");
        let outside = directory.path().join("outside");
        fs::create_dir_all(host.join("wp-content/plugins/example")).unwrap();
        fs::create_dir_all(host.join("wp-content/uploads")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(host.join("wp-config.php"), "<?php\n").unwrap();
        fs::create_dir_all(host.join("wp-content/themes")).unwrap();
        fs::create_dir_all(host.join("wp-admin")).unwrap();
        std::os::unix::fs::symlink(&outside, host.join("wp-includes")).unwrap();
        let workspace = host.join(".ddev-workspaces/worktrees/one");
        fs::create_dir_all(&workspace).unwrap();
        state::ensure_state(&host).unwrap();
        let config = wordpress_config("wp-content/plugins/example");
        let paths = RuntimePaths {
            repository_root: workspace.clone(),
            host_root: host.clone(),
            worktree_root: host.join(".ddev-workspaces/worktrees"),
        };

        let error = prepare(
            &config,
            &paths,
            &workspace,
            "fixture-one",
            "dw_one",
            "https://one.example",
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside authorized root"));
    }

    #[cfg(unix)]
    #[test]
    fn nested_components_create_missing_ancestor_directories_in_the_view() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("view");
        let workspace = directory.path().join("workspace");
        fs::create_dir_all(source.join("vendor/component")).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&workspace).unwrap();

        link_tree_except(
            &source,
            &destination,
            Path::new("vendor/component"),
            &workspace,
            directory.path(),
        )
        .unwrap();

        assert_eq!(
            fs::canonicalize(destination.join("vendor/component")).unwrap(),
            fs::canonicalize(workspace).unwrap()
        );
    }
}
