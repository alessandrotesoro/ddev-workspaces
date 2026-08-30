use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    message: String,
    exit_code: u8,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: 1,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: 2,
        }
    }

    pub fn exit_code(&self) -> u8 {
        self.exit_code
    }
}

impl Display for ToolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(error: std::io::Error) -> Self {
        Self::new(format!("I/O error: {error}"))
    }
}

impl From<toml::de::Error> for ToolError {
    fn from(error: toml::de::Error) -> Self {
        Self::new(format!("TOML parse error: {error}"))
    }
}

impl From<toml::ser::Error> for ToolError {
    fn from(error: toml::ser::Error) -> Self {
        Self::new(format!("TOML serialization error: {error}"))
    }
}

pub type ToolResult<T> = Result<T, ToolError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub sensitive: bool,
    pub mutating: bool,
}

impl CommandRequest {
    pub fn new<I, S>(program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: None,
            env: Vec::new(),
            sensitive: false,
            mutating: false,
        }
    }

    pub fn cwd(mut self, cwd: &Path) -> Self {
        self.cwd = Some(cwd.to_path_buf());
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    pub fn mutating(mut self) -> Self {
        self.mutating = true;
        self
    }

    pub fn sensitive(mut self) -> Self {
        self.sensitive = true;
        self
    }

    pub fn display(&self) -> String {
        if self.sensitive {
            format!("{} [sensitive command]", self.program)
        } else {
            let args = self
                .args
                .iter()
                .map(|argument| shell_quote_for_display(argument))
                .collect::<Vec<_>>()
                .join(" ");
            if args.is_empty() {
                self.program.clone()
            } else {
                format!("{} {args}", self.program)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }

    pub fn dry_run() -> Self {
        Self {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

pub trait CommandRunner {
    fn run(&mut self, request: &CommandRequest) -> ToolResult<CommandOutput>;
}

#[derive(Debug, Default)]
pub struct RealCommandRunner {
    pub dry_run: bool,
}

impl RealCommandRunner {
    pub fn new(dry_run: bool) -> Self {
        Self { dry_run }
    }
}

impl CommandRunner for RealCommandRunner {
    fn run(&mut self, request: &CommandRequest) -> ToolResult<CommandOutput> {
        if self.dry_run && request.mutating {
            return Ok(CommandOutput::dry_run());
        }

        let mut command = Command::new(&request.program);
        command.args(&request.args);
        command.envs(request.env.iter().map(|(name, value)| (name, value)));
        if let Some(cwd) = &request.cwd {
            command.current_dir(cwd);
        }
        let process_error = |error: std::io::Error| {
            let cwd = request
                .cwd
                .as_deref()
                .map(|path| format!(" in {}", path.display()))
                .unwrap_or_default();
            ToolError::new(format!("could not run {}{cwd}: {error}", request.display()))
        };
        if request.sensitive {
            let status = command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(process_error)?;
            return Ok(CommandOutput {
                status: status.code().unwrap_or(1),
                stdout: String::new(),
                stderr: String::new(),
            });
        }
        let output = command
            .stdin(Stdio::null())
            .output()
            .map_err(process_error)?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn shell_quote_for_display(argument: &str) -> String {
    if argument
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._/-".contains(character))
    {
        argument.to_owned()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn sensitive_requests_redact_arguments_in_display() {
        let request = CommandRequest::new("printf", ["secret-value"]).sensitive();

        assert_eq!(request.display(), "printf [sensitive command]");
    }

    #[test]
    fn sensitive_process_output_is_not_retained() {
        let mut runner = RealCommandRunner::new(false);
        let request = CommandRequest::new("printf", ["secret-value"]).sensitive();

        let output = runner.run(&request).expect("printf should run");

        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn dry_run_runner_skips_mutation_without_executing_it() {
        let mut runner = RealCommandRunner::new(true);
        let request = CommandRequest::new("definitely-not-a-command", std::iter::empty::<String>())
            .mutating();

        let output = runner.run(&request).expect("dry-run should succeed");

        assert!(output.success());
    }

    #[test]
    fn request_builders_preserve_the_process_contract() {
        let directory = tempdir().expect("temporary directory");
        let request = CommandRequest::new("printf", ["two words", "it's-safe"])
            .cwd(directory.path())
            .env("FIXTURE_VALUE", "configured")
            .mutating();

        assert_eq!(request.cwd.as_deref(), Some(directory.path()));
        assert_eq!(
            request.env,
            [("FIXTURE_VALUE".to_owned(), "configured".to_owned())]
        );
        assert!(request.mutating);
        assert_eq!(request.display(), "printf 'two words' 'it'\\''s-safe'");
        assert_eq!(
            CommandRequest::new("pwd", std::iter::empty::<String>()).display(),
            "pwd"
        );
    }

    #[test]
    fn real_runner_honors_working_directory_and_environment() {
        let directory = tempdir().expect("temporary directory");
        let mut runner = RealCommandRunner::new(false);

        let pwd = runner
            .run(&CommandRequest::new("pwd", std::iter::empty::<String>()).cwd(directory.path()))
            .expect("pwd should run");
        let environment = runner
            .run(
                &CommandRequest::new("printenv", ["DDEV_WORKSPACES_TEST_VALUE"])
                    .env("DDEV_WORKSPACES_TEST_VALUE", "configured"),
            )
            .expect("printenv should run");

        assert!(pwd.success());
        assert_eq!(
            fs::canonicalize(pwd.stdout.trim()).expect("reported working directory"),
            fs::canonicalize(directory.path()).expect("fixture working directory")
        );
        assert_eq!(environment.stdout.trim(), "configured");
    }

    #[test]
    fn process_start_errors_report_the_request_without_leaking_sensitive_arguments() {
        let directory = tempdir().expect("temporary directory");
        let mut runner = RealCommandRunner::new(false);
        let error = runner
            .run(
                &CommandRequest::new("definitely-not-a-command", ["secret"])
                    .cwd(directory.path())
                    .sensitive(),
            )
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("definitely-not-a-command [sensitive command]")
        );
        assert!(
            error
                .to_string()
                .contains(&directory.path().display().to_string())
        );
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn errors_and_outputs_expose_stable_cli_status() {
        let usage = ToolError::usage("bad arguments");
        let failure = ToolError::new("failed");
        let io_error: ToolError = std::io::Error::other("disk unavailable").into();
        let output = CommandOutput {
            status: 7,
            stdout: String::new(),
            stderr: "failure".to_owned(),
        };

        assert_eq!(usage.exit_code(), 2);
        assert_eq!(usage.to_string(), "bad arguments");
        assert_eq!(failure.exit_code(), 1);
        assert!(io_error.to_string().contains("I/O error: disk unavailable"));
        assert!(!output.success());
        assert!(CommandOutput::dry_run().success());
    }
}
