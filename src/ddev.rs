use std::fs;
use std::path::{Path, PathBuf};
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
        assert!(write_override(repository.path(), &app, "dw-fixture--task", &mut runner).is_err());

        fs::create_dir(app.join(".ddev")).expect("DDEV directory");
        assert!(write_override(repository.path(), &app, "dw-fixture--task", &mut runner).is_err());

        fs::write(app.join(".ddev/config.yaml"), "type: php\n").expect("DDEV config");
        runner.responses.push(CommandOutput {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        });
        let override_path =
            write_override(repository.path(), &app, "dw-fixture--task", &mut runner)
                .expect("ignored override should be created");
        assert_eq!(
            fs::read_to_string(&override_path).expect("override contents"),
            "name: dw-fixture--task\n"
        );
        assert!(write_override(repository.path(), &app, "dw-fixture--task", &mut runner).is_err());
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
}
