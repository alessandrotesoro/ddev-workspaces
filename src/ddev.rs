use std::fs;
use std::path::{Path, PathBuf};

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
    let request = CommandRequest::new("ddev", ["list", "--json-output"]).cwd(cwd);
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
    let mut warnings = Vec::new();
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
    if !config::is_ignored(repo_root, &override_path, runner)? {
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
    file.write_all(format!("name: {expected_name}\n").as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            ToolError::new(format!(
                "cannot finish owned DDEV override {}: {error}",
                override_path.display()
            ))
        })?;
    Ok(override_path)
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
                executed: true,
            }))
        }
    }

    #[test]
    fn names_are_deterministic_and_limited_to_dns_label_size() {
        let name = expected_name("filebean", "invoice-fix").expect("valid DDEV name");

        assert_eq!(name, "dw-filebean--invoice-fix");
        assert!(expected_name(&"a".repeat(60), "task").is_err());
    }

    #[test]
    fn exact_identity_requires_running_and_healthy_mutagen() {
        let inspection = DdevInspection {
            entries: vec![DdevProject {
                name: "dw-filebean--task".to_owned(),
                approot: "/tmp/workspace".to_owned(),
                status: "running".to_owned(),
                mutagen_enabled: true,
                mutagen_status: "ok".to_owned(),
            }],
            warnings: Vec::new(),
        };

        let project = require_ready_identity(
            &inspection,
            "dw-filebean--task",
            &PathBuf::from("/tmp/workspace"),
        )
        .expect("identity should be ready");

        assert_eq!(project.name, "dw-filebean--task");
    }

    #[test]
    fn conflicting_name_or_path_is_rejected_before_start() {
        let inspection = DdevInspection {
            entries: vec![DdevProject {
                name: "dw-filebean--other".to_owned(),
                approot: "/tmp/workspace".to_owned(),
                status: "running".to_owned(),
                mutagen_enabled: false,
                mutagen_status: String::new(),
            }],
            warnings: Vec::new(),
        };

        let error = inspect_new_identity(
            &inspection,
            "dw-filebean--task",
            &PathBuf::from("/tmp/workspace"),
        )
        .unwrap_err();

        assert!(error.to_string().contains("already registered"));
    }

    #[test]
    fn malformed_json_fails_closed_without_using_partial_state() {
        let mut runner = FakeRunner {
            requests: Vec::new(),
            responses: vec![CommandOutput {
                status: 0,
                stdout: "{not-json".to_owned(),
                stderr: String::new(),
                executed: true,
            }],
        };

        let error = list(&mut runner, Path::new("/tmp")).unwrap_err();

        assert!(error.to_string().contains("unsupported JSON"));
    }

    #[test]
    fn data_removal_uses_only_stop_remove_data_unlist() {
        let mut runner = FakeRunner::default();

        stop_unlist(
            Path::new("/tmp/workspace"),
            "dw-filebean--task",
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
                "dw-filebean--task".to_owned()
            ]
        );
        assert!(!runner.requests[0].args.iter().any(|arg| arg == "delete"));
    }
}
