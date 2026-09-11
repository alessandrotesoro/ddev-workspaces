use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::command::{CommandRequest, CommandRunner, ToolError, ToolResult};
use crate::config::{DatabaseMode, ProjectConfig};
use crate::ddev::HostIdentity;
use crate::runtime::SignalGuard;

const OWNERSHIP_TABLE: &str = "__ddev_workspaces_ownership";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseEngine {
    MySql,
    MariaDb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseContext {
    pub host_root: PathBuf,
    pub engine: DatabaseEngine,
    pub source: String,
}

impl DatabaseContext {
    pub fn from_host(
        host_root: &Path,
        identity: &HostIdentity,
        config: &ProjectConfig,
    ) -> ToolResult<Self> {
        Self::from_host_source(host_root, identity, config.database.source.as_deref())
    }

    pub fn from_host_source(
        host_root: &Path,
        identity: &HostIdentity,
        source: Option<&str>,
    ) -> ToolResult<Self> {
        let engine = match identity.project.database_type.to_ascii_lowercase().as_str() {
            "mysql" => DatabaseEngine::MySql,
            "mariadb" => DatabaseEngine::MariaDb,
            value => {
                return Err(ToolError::new(format!(
                    "database engine {value} is unsupported; use MySQL or MariaDB"
                )));
            }
        };
        Ok(Self {
            host_root: host_root.to_path_buf(),
            engine,
            source: source.unwrap_or("db").to_owned(),
        })
    }

    pub fn create<R: CommandRunner>(&self, name: &str, runner: &mut R) -> ToolResult<()> {
        validate_name(name)?;
        self.sql(&format!("CREATE DATABASE {name}"), runner, true)
            .map(|_| ())
    }

    pub fn drop<R: CommandRunner>(&self, name: &str, runner: &mut R) -> ToolResult<()> {
        validate_name(name)?;
        self.sql(&format!("DROP DATABASE IF EXISTS {name}"), runner, true)
            .map(|_| ())
    }

    pub fn exists<R: CommandRunner>(&self, name: &str, runner: &mut R) -> ToolResult<bool> {
        validate_name(name)?;
        let output = self.sql(
            &format!(
                "SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = '{name}'"
            ),
            runner,
            false,
        )?;
        Ok(output
            .stdout
            .lines()
            .any(|line| line.trim() == name || line.trim() == "1"))
    }

    pub fn prepare<R: CommandRunner>(
        &self,
        name: &str,
        mode: DatabaseMode,
        ownership_token: &str,
        runner: &mut R,
    ) -> ToolResult<()> {
        validate_name(name)?;
        validate_ownership_token(ownership_token)?;
        match mode {
            DatabaseMode::Fresh => {
                self.create(name, runner)?;
                if let Err(error) = self.mark_owned(name, ownership_token, runner) {
                    let drop_result = self.drop_owned(name, ownership_token, runner);
                    return Err(with_drop_error(
                        ToolError::new(format!(
                            "workspace database {name} was created but its ownership marker could not be recorded: {error}"
                        )),
                        drop_result,
                    ));
                }
                Ok(())
            }
            DatabaseMode::Copy => self.copy(&self.source, name, ownership_token, runner),
        }
    }

    fn mark_owned<R: CommandRunner>(
        &self,
        name: &str,
        ownership_token: &str,
        runner: &mut R,
    ) -> ToolResult<()> {
        validate_name(name)?;
        validate_ownership_token(ownership_token)?;
        self.sql_in_database(
            name,
            &format!(
                "CREATE TABLE IF NOT EXISTS {OWNERSHIP_TABLE} (token VARCHAR(128) NOT NULL PRIMARY KEY)"
            ),
            runner,
            true,
        )?;
        self.sql_in_database(
            name,
            &format!("DELETE FROM {OWNERSHIP_TABLE}"),
            runner,
            true,
        )?;
        self.sql_in_database(
            name,
            &format!("INSERT INTO {OWNERSHIP_TABLE} (token) VALUES ('{ownership_token}')"),
            runner,
            true,
        )?;
        Ok(())
    }

    pub fn owns<R: CommandRunner>(
        &self,
        name: &str,
        ownership_token: &str,
        runner: &mut R,
    ) -> ToolResult<bool> {
        validate_name(name)?;
        validate_ownership_token(ownership_token)?;
        let output = self.sql_in_database(
            name,
            &format!("SELECT token FROM {OWNERSHIP_TABLE} WHERE token = '{ownership_token}'"),
            runner,
            false,
        )?;
        Ok(output
            .stdout
            .lines()
            .any(|line| line.trim() == ownership_token))
    }

    pub fn drop_owned<R: CommandRunner>(
        &self,
        name: &str,
        ownership_token: &str,
        runner: &mut R,
    ) -> ToolResult<()> {
        if !self.owns(name, ownership_token, runner)? {
            return Err(ToolError::new(format!(
                "workspace database {name} is missing its ownership marker; refusing deletion; inspect it manually"
            )));
        }
        self.drop(name, runner)
    }

    pub fn copy<R: CommandRunner>(
        &self,
        source: &str,
        destination: &str,
        ownership_token: &str,
        runner: &mut R,
    ) -> ToolResult<()> {
        validate_name(source)?;
        validate_name(destination)?;
        validate_ownership_token(ownership_token)?;
        let signal_guard = SignalGuard::install()?;
        let dump = temporary_dump_path()?;
        let _dump_guard = DumpGuard { path: dump.clone() };
        let export = match runner.run(
            &CommandRequest::new(
                "ddev",
                [
                    "export-db",
                    &format!("--database={source}"),
                    "--gzip",
                    &format!("--file={}", dump.display()),
                ],
            )
            .cwd(&self.host_root)
            .mutating()
            .sensitive(),
        ) {
            Ok(output) => output,
            Err(error) => {
                return Err(with_cleanup_error(error, &dump));
            }
        };
        if signal_guard.interrupted() {
            return Err(with_cleanup_error(
                ToolError::new("database copy interrupted during export"),
                &dump,
            ));
        }
        if !export.success() {
            return Err(with_cleanup_error(
                ToolError::new(
                    "DDEV could not export the configured source database; workspace database was not prepared",
                ),
                &dump,
            ));
        }
        let create_result = self.create(destination, runner);
        if let Err(error) = create_result {
            return Err(with_cleanup_error(error, &dump));
        }
        if signal_guard.interrupted() {
            let drop_result = self.drop_owned(destination, ownership_token, runner);
            return Err(with_cleanup_and_drop_error(
                ToolError::new("database copy interrupted before import"),
                &dump,
                drop_result,
            ));
        }
        let import = match runner.run(
            &CommandRequest::new(
                "ddev",
                [
                    "import-db",
                    &format!("--database={destination}"),
                    "--no-drop",
                    "--no-progress",
                    &format!("--file={}", dump.display()),
                ],
            )
            .cwd(&self.host_root)
            .mutating()
            .sensitive(),
        ) {
            Ok(output) => output,
            Err(error) => {
                let drop_result = self.drop_owned(destination, ownership_token, runner);
                return Err(with_cleanup_and_drop_error(error, &dump, drop_result));
            }
        };
        if signal_guard.interrupted() {
            let drop_result = self.drop_owned(destination, ownership_token, runner);
            return Err(with_cleanup_and_drop_error(
                ToolError::new("database copy interrupted during import"),
                &dump,
                drop_result,
            ));
        }
        if !import.success() {
            let drop_result = self.drop_owned(destination, ownership_token, runner);
            return Err(with_cleanup_and_drop_error(
                ToolError::new(
                    "DDEV could not import the source database into the workspace database; inspect the retained ownership record before retrying",
                ),
                &dump,
                drop_result,
            ));
        }
        if let Err(error) = self.mark_owned(destination, ownership_token, runner) {
            let drop_result = self.drop_owned(destination, ownership_token, runner);
            return Err(with_cleanup_and_drop_error(
                ToolError::new(format!(
                    "workspace database {destination} was imported but its ownership marker could not be recorded: {error}"
                )),
                &dump,
                drop_result,
            ));
        }
        if let Err(cleanup_error) = cleanup_dump(&dump) {
            let drop_result = self.drop_owned(destination, ownership_token, runner);
            let mut message = cleanup_error.to_string();
            match drop_result {
                Ok(()) => message.push_str(
                    "\nThe workspace database was imported but was dropped because the temporary dump could not be cleaned up.",
                ),
                Err(drop_error) => {
                    message.push_str(
                        "\nThe workspace database was imported, but it could not be dropped after the temporary dump cleanup failed; it may still exist and requires explicit inspection.",
                    );
                    message.push_str(&format!(
                        "\nAdditionally, the workspace database cleanup failed: {drop_error}"
                    ));
                }
            }
            return Err(ToolError::new(message));
        }
        Ok(())
    }

    fn sql<R: CommandRunner>(
        &self,
        statement: &str,
        runner: &mut R,
        mutating: bool,
    ) -> ToolResult<crate::command::CommandOutput> {
        self.sql_with_database(None, statement, runner, mutating)
    }

    fn sql_in_database<R: CommandRunner>(
        &self,
        database: &str,
        statement: &str,
        runner: &mut R,
        mutating: bool,
    ) -> ToolResult<crate::command::CommandOutput> {
        self.sql_with_database(Some(database), statement, runner, mutating)
    }

    fn sql_with_database<R: CommandRunner>(
        &self,
        database: Option<&str>,
        statement: &str,
        runner: &mut R,
        mutating: bool,
    ) -> ToolResult<crate::command::CommandOutput> {
        let client = match self.engine {
            DatabaseEngine::MySql => "mysql",
            DatabaseEngine::MariaDb => "mariadb",
        };
        let mut args = vec![
            "exec".to_owned(),
            "--raw".to_owned(),
            "--service".to_owned(),
            "db".to_owned(),
            "--".to_owned(),
            client.to_owned(),
            "--user=root".to_owned(),
            "--password=root".to_owned(),
        ];
        if let Some(database) = database {
            args.push(format!("--database={database}"));
        }
        args.push("--batch".to_owned());
        args.push("--skip-column-names".to_owned());
        args.push(format!("--execute={statement}"));
        let mut request = CommandRequest::new("ddev", args)
            .cwd(&self.host_root)
            .redact();
        if mutating {
            request = request.mutating();
        }
        let output = runner.run(&request)?;
        if output.success() {
            Ok(output)
        } else {
            Err(ToolError::new(
                "DDEV database command failed; inspect the shared host database manually",
            ))
        }
    }
}

fn cleanup_dump(path: &Path) -> ToolResult<()> {
    let mut errors = Vec::new();
    let Some(parent) = path.parent() else {
        return Err(ToolError::new(
            "temporary database dump path has no parent directory",
        ));
    };
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            errors.push(format!(
                "temporary database directory {} is not a regular directory",
                parent.display()
            ));
        }
        Ok(_) => {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => errors.push(format!("cannot remove {}: {error}", path.display())),
            }
            if let Err(error) = fs::remove_dir_all(parent) {
                errors.push(format!(
                    "cannot remove temporary database directory {}: {error}",
                    parent.display()
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => errors.push(format!(
            "cannot inspect temporary database directory {}: {error}",
            parent.display()
        )),
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "temporary database dump cleanup failed: {}",
            errors.join("; ")
        )))
    }
}

fn with_cleanup_error(error: ToolError, dump: &Path) -> ToolError {
    match cleanup_dump(dump) {
        Ok(()) => error,
        Err(cleanup_error) => ToolError::new(format!("{error}\n{cleanup_error}")),
    }
}

fn with_drop_error(error: ToolError, drop_result: ToolResult<()>) -> ToolError {
    match drop_result {
        Ok(()) => error,
        Err(drop_error) => ToolError::new(format!(
            "{error}\nThe workspace database could not be dropped and may still exist; inspect it before retrying.\nAdditionally, the workspace database cleanup failed: {drop_error}"
        )),
    }
}

fn with_cleanup_and_drop_error(
    error: ToolError,
    dump: &Path,
    drop_result: ToolResult<()>,
) -> ToolError {
    let mut message = error.to_string();
    if let Err(cleanup_error) = cleanup_dump(dump) {
        message.push_str(&format!("\n{cleanup_error}"));
    }
    if let Err(drop_error) = drop_result {
        message.push_str(
            "\nThe workspace database could not be dropped and may still exist; inspect it before retrying.",
        );
        message.push_str(&format!(
            "\nAdditionally, the workspace database cleanup failed: {drop_error}"
        ));
    }
    ToolError::new(message)
}

struct DumpGuard {
    path: PathBuf,
}

impl Drop for DumpGuard {
    fn drop(&mut self) {
        let _ = cleanup_dump(&self.path);
    }
}

fn validate_name(name: &str) -> ToolResult<()> {
    crate::config::validate_database_name("database name", name)
}

fn validate_ownership_token(token: &str) -> ToolResult<()> {
    if token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ToolError::new(
            "database ownership token contains unsupported characters",
        ));
    }
    Ok(())
}

fn temporary_dump_path() -> ToolResult<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ToolError::new(format!("system clock error: {error}")))?
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("ddev-workspaces-db-{}-{nonce}", std::process::id()));
    fs::create_dir(&directory).map_err(|error| {
        ToolError::new(format!(
            "cannot create private temporary database directory {}: {error}",
            directory.display()
        ))
    })?;
    let path = directory.join("database.sql.gz");
    let result = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&directory)?.permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&directory, permissions)?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = file.metadata()?.permissions();
            permissions.set_mode(0o600);
            file.set_permissions(permissions)?;
        }
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = result {
        let error = ToolError::new(format!(
            "cannot create private temporary database dump {}: {error}",
            path.display()
        ));
        return match cleanup_dump(&path) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(ToolError::new(format!("{error}\n{cleanup_error}"))),
        };
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandOutput, CommandRunner};
    use crate::config::{DatabaseConfig, RuntimeConfig};
    use crate::ddev::{DdevProject, HostIdentity};
    use std::cell::RefCell;
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[derive(Default)]
    struct FakeRunner {
        requests: RefCell<Vec<CommandRequest>>,
        owned_databases: RefCell<HashMap<String, String>>,
        drop_failure: bool,
        export_failure: bool,
        import_failure: bool,
        leave_dump_sibling: bool,
    }

    impl CommandRunner for FakeRunner {
        fn run(&mut self, request: &CommandRequest) -> ToolResult<CommandOutput> {
            self.requests.borrow_mut().push(request.clone());
            if self.drop_failure
                && request
                    .args
                    .iter()
                    .any(|arg| arg == "--execute=DROP DATABASE IF EXISTS dw_fixture_feature")
            {
                return Ok(CommandOutput {
                    status: 1,
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            if let Some(statement) = request
                .args
                .iter()
                .find_map(|argument| argument.strip_prefix("--execute="))
                && statement.starts_with("DROP DATABASE IF EXISTS ")
                && let Some(database) = statement.strip_prefix("DROP DATABASE IF EXISTS ")
            {
                self.owned_databases.borrow_mut().remove(database);
            }
            let database = request
                .args
                .iter()
                .find_map(|argument| argument.strip_prefix("--database=").map(str::to_owned));
            if let Some(statement) = request
                .args
                .iter()
                .find_map(|argument| argument.strip_prefix("--execute="))
                && statement.contains("SELECT token FROM __ddev_workspaces_ownership")
            {
                let token = statement
                    .split_once("WHERE token = '")
                    .and_then(|(_, value)| value.split_once('\'').map(|(token, _)| token))
                    .unwrap_or_default();
                let owned = database.as_deref().is_some_and(|database| {
                    self.owned_databases
                        .borrow()
                        .get(database)
                        .is_some_and(|owned_token| owned_token == token)
                });
                return Ok(CommandOutput {
                    status: 0,
                    stdout: if owned {
                        format!("{token}\n")
                    } else {
                        String::new()
                    },
                    stderr: String::new(),
                });
            }
            if let Some(statement) = request
                .args
                .iter()
                .find_map(|argument| argument.strip_prefix("--execute="))
                && statement.contains("INSERT INTO __ddev_workspaces_ownership")
            {
                let token = statement
                    .split_once("VALUES ('")
                    .and_then(|(_, value)| value.split_once('\'').map(|(token, _)| token))
                    .unwrap_or_default();
                if let Some(database) = database {
                    self.owned_databases
                        .borrow_mut()
                        .insert(database, token.to_owned());
                }
                return Ok(CommandOutput {
                    status: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            if self.export_failure && request.args.first().is_some_and(|arg| arg == "export-db") {
                return Ok(CommandOutput {
                    status: 1,
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            if self.import_failure && request.args.first().is_some_and(|arg| arg == "import-db") {
                return Ok(CommandOutput {
                    status: 1,
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            if self.leave_dump_sibling && request.args.first().is_some_and(|arg| arg == "export-db")
            {
                let dump = request
                    .args
                    .iter()
                    .find_map(|arg| arg.strip_prefix("--file="))
                    .map(PathBuf::from)
                    .expect("export request has a dump path");
                fs::write(
                    dump.parent().unwrap().join("leftover-during-cleanup"),
                    "sentinel",
                )
                .unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut permissions =
                        fs::metadata(dump.parent().unwrap()).unwrap().permissions();
                    permissions.set_mode(0o500);
                    fs::set_permissions(dump.parent().unwrap(), permissions).unwrap();
                }
            }
            Ok(CommandOutput {
                status: 0,
                stdout: "dw_fixture_feature\n".to_owned(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn database_commands_use_the_host_db_service_and_no_shell() {
        let directory = tempdir().unwrap();
        let identity = HostIdentity {
            project: DdevProject {
                database_type: "mariadb".to_owned(),
                ..DdevProject::default()
            },
            hostname: "fixture.ddev.site".to_owned(),
        };
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
            environment: None,
            carry: Vec::new(),
            provision: Vec::new(),
            checks: Vec::new(),
            writable: Vec::new(),
        };
        let context = DatabaseContext::from_host(directory.path(), &identity, &config).unwrap();
        let mut runner = FakeRunner::default();
        assert!(context.exists("dw_fixture_feature", &mut runner).unwrap());
        let request = &runner.requests.borrow()[0];
        assert_eq!(request.program, "ddev");
        assert!(request.args.contains(&"--service".to_owned()));
        assert!(request.args.contains(&"db".to_owned()));
        assert!(request.redact_display);
    }

    #[test]
    fn database_creation_is_strict_and_never_idempotent() {
        let directory = tempdir().unwrap();
        let identity = HostIdentity {
            project: DdevProject {
                database_type: "mariadb".to_owned(),
                ..DdevProject::default()
            },
            hostname: "fixture.ddev.site".to_owned(),
        };
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
            environment: None,
            carry: Vec::new(),
            provision: Vec::new(),
            checks: Vec::new(),
            writable: Vec::new(),
        };
        let context = DatabaseContext::from_host(directory.path(), &identity, &config).unwrap();
        let mut runner = FakeRunner::default();
        context.create("dw_fixture_feature", &mut runner).unwrap();
        let request = runner.requests.borrow().last().unwrap().clone();
        assert!(
            request
                .args
                .contains(&"--execute=CREATE DATABASE dw_fixture_feature".to_owned())
        );
        assert!(!request.args.iter().any(|arg| arg.contains("IF NOT EXISTS")));
    }

    #[test]
    fn database_deletion_requires_the_matching_database_marker() {
        let directory = tempdir().unwrap();
        let identity = HostIdentity {
            project: DdevProject {
                database_type: "mariadb".to_owned(),
                ..DdevProject::default()
            },
            hostname: "fixture.ddev.site".to_owned(),
        };
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
            environment: None,
            carry: Vec::new(),
            provision: Vec::new(),
            checks: Vec::new(),
            writable: Vec::new(),
        };
        let context = DatabaseContext::from_host(directory.path(), &identity, &config).unwrap();
        let mut runner = FakeRunner::default();
        runner
            .owned_databases
            .borrow_mut()
            .insert("dw_fixture_feature".to_owned(), "other-token".to_owned());

        let error = context
            .drop_owned("dw_fixture_feature", "fixture-token", &mut runner)
            .unwrap_err();
        assert!(error.to_string().contains("missing its ownership marker"));
        assert!(!runner.requests.borrow().iter().any(|request| {
            request
                .args
                .iter()
                .any(|arg| arg == "--execute=DROP DATABASE IF EXISTS dw_fixture_feature")
        }));
    }

    #[test]
    fn database_copy_failure_does_not_raw_drop_an_unmarked_database() {
        let directory = tempdir().unwrap();
        let identity = HostIdentity {
            project: DdevProject {
                database_type: "mariadb".to_owned(),
                ..DdevProject::default()
            },
            hostname: "fixture.ddev.site".to_owned(),
        };
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
            environment: None,
            carry: Vec::new(),
            provision: Vec::new(),
            checks: Vec::new(),
            writable: Vec::new(),
        };
        let context = DatabaseContext::from_host(directory.path(), &identity, &config).unwrap();
        let mut runner = FakeRunner {
            import_failure: true,
            ..Default::default()
        };

        let error = context
            .copy("db", "dw_fixture_feature", "fixture-token", &mut runner)
            .unwrap_err();
        assert!(error.to_string().contains("could not be dropped"));
        assert!(!runner.requests.borrow().iter().any(|request| {
            request
                .args
                .iter()
                .any(|arg| arg == "--execute=DROP DATABASE IF EXISTS dw_fixture_feature")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn database_copy_surfaces_temporary_dump_cleanup_failures() {
        let directory = tempdir().unwrap();
        let identity = HostIdentity {
            project: DdevProject {
                database_type: "mariadb".to_owned(),
                ..DdevProject::default()
            },
            hostname: "fixture.ddev.site".to_owned(),
        };
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
            environment: None,
            carry: Vec::new(),
            provision: Vec::new(),
            checks: Vec::new(),
            writable: Vec::new(),
        };
        let context = DatabaseContext::from_host(directory.path(), &identity, &config).unwrap();
        let mut runner = FakeRunner {
            leave_dump_sibling: true,
            ..Default::default()
        };

        let error = context
            .copy("db", "dw_fixture_feature", "fixture-token", &mut runner)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("temporary database dump cleanup failed")
        );
        assert!(
            error
                .to_string()
                .contains("workspace database was imported but was dropped")
        );
        assert!(runner.requests.borrow().iter().any(|request| {
            request
                .args
                .iter()
                .any(|arg| arg == "--execute=DROP DATABASE IF EXISTS dw_fixture_feature")
        }));

        let dump = runner
            .requests
            .borrow()
            .iter()
            .find(|request| request.args.first().is_some_and(|arg| arg == "export-db"))
            .and_then(|request| {
                request
                    .args
                    .iter()
                    .find_map(|arg| arg.strip_prefix("--file="))
            })
            .map(PathBuf::from)
            .unwrap();
        let leftover = dump.parent().unwrap().join("leftover-during-cleanup");
        assert!(leftover.is_file());
        let mut permissions = fs::metadata(dump.parent().unwrap()).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(dump.parent().unwrap(), permissions).unwrap();
        fs::remove_file(&dump).unwrap();
        fs::remove_file(leftover).unwrap();
        fs::remove_dir(dump.parent().unwrap()).unwrap();

        let mut drop_failure_runner = FakeRunner {
            drop_failure: true,
            leave_dump_sibling: true,
            ..Default::default()
        };
        let error = context
            .copy(
                "db",
                "dw_fixture_feature",
                "fixture-token",
                &mut drop_failure_runner,
            )
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("could not be dropped"));
        assert!(message.contains("may still exist"));
        let dump = drop_failure_runner
            .requests
            .borrow()
            .iter()
            .find(|request| request.args.first().is_some_and(|arg| arg == "export-db"))
            .and_then(|request| {
                request
                    .args
                    .iter()
                    .find_map(|arg| arg.strip_prefix("--file="))
            })
            .map(PathBuf::from)
            .unwrap();
        let leftover = dump.parent().unwrap().join("leftover-during-cleanup");
        assert!(leftover.is_file());
        let mut permissions = fs::metadata(dump.parent().unwrap()).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(dump.parent().unwrap(), permissions).unwrap();
        fs::remove_file(&dump).unwrap();
        fs::remove_file(leftover).unwrap();
        fs::remove_dir(dump.parent().unwrap()).unwrap();

        let mut import_failure_runner = FakeRunner {
            import_failure: true,
            ..Default::default()
        };
        let error = context
            .copy(
                "db",
                "dw_fixture_feature",
                "fixture-token",
                &mut import_failure_runner,
            )
            .unwrap_err();
        assert!(error.to_string().contains("could not import"));
        let dump = import_failure_runner
            .requests
            .borrow()
            .iter()
            .find(|request| request.args.first().is_some_and(|arg| arg == "export-db"))
            .and_then(|request| {
                request
                    .args
                    .iter()
                    .find_map(|arg| arg.strip_prefix("--file="))
            })
            .map(PathBuf::from)
            .unwrap();
        assert!(!dump.exists());
        assert!(!dump.parent().unwrap().exists());

        let mut successful_runner = FakeRunner::default();
        context
            .copy(
                "db",
                "dw_fixture_feature",
                "fixture-token",
                &mut successful_runner,
            )
            .unwrap();
        let dump = successful_runner
            .requests
            .borrow()
            .iter()
            .find(|request| request.args.first().is_some_and(|arg| arg == "export-db"))
            .and_then(|request| {
                request
                    .args
                    .iter()
                    .find_map(|arg| arg.strip_prefix("--file="))
            })
            .map(PathBuf::from)
            .unwrap();
        assert!(!dump.exists());
        assert!(!dump.parent().unwrap().exists());

        let mut export_failure_runner = FakeRunner {
            export_failure: true,
            ..Default::default()
        };
        let error = context
            .copy(
                "db",
                "dw_fixture_feature",
                "fixture-token",
                &mut export_failure_runner,
            )
            .unwrap_err();
        assert!(error.to_string().contains("could not export"));
        let dump = export_failure_runner
            .requests
            .borrow()
            .iter()
            .find(|request| request.args.first().is_some_and(|arg| arg == "export-db"))
            .and_then(|request| {
                request
                    .args
                    .iter()
                    .find_map(|arg| arg.strip_prefix("--file="))
            })
            .map(PathBuf::from)
            .unwrap();
        assert!(!dump.exists());
        assert!(!dump.parent().unwrap().exists());
    }
}
