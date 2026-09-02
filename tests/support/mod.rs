#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::{TempDir, tempdir};

pub fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ddev-workspaces"))
}

pub fn run_git(root: &Path, arguments: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("git must be installed for local fixture tests")
}

pub fn init_repo() -> TempDir {
    let directory = tempdir().expect("temporary repository directory");
    assert!(
        run_git(directory.path(), &["init", "-b", "main"])
            .status
            .success()
    );
    assert!(
        run_git(directory.path(), &["config", "user.name", "Fixture User"])
            .status
            .success()
    );
    assert!(
        run_git(
            directory.path(),
            &["config", "user.email", "fixture@example.test"],
        )
        .status
        .success()
    );
    fs::write(directory.path().join("README.md"), "fixture\n").expect("fixture file");
    fs::write(
        directory.path().join(".gitignore"),
        ".worktrees/\n.ddev/config.ddev-workspaces.yaml\n.env\n",
    )
    .expect("fixture ignore file");
    commit(directory.path(), "initial fixture");
    directory
}

pub fn commit(root: &Path, message: &str) {
    assert!(run_git(root, &["add", "--all"]).status.success());
    assert!(run_git(root, &["commit", "-m", message]).status.success());
}

pub fn init_repo_with_origin() -> (TempDir, TempDir) {
    let repository = init_repo();
    let remote = tempdir().expect("bare remote directory");
    assert!(
        Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(remote.path())
            .output()
            .expect("git bare init")
            .status
            .success()
    );
    assert!(
        run_git(
            repository.path(),
            &[
                "remote",
                "add",
                "origin",
                remote.path().to_str().expect("remote path")
            ],
        )
        .status
        .success()
    );
    assert!(
        run_git(repository.path(), &["push", "origin", "main"])
            .status
            .success()
    );
    assert!(
        Command::new("git")
            .arg("--git-dir")
            .arg(remote.path())
            .args(["symbolic-ref", "HEAD", "refs/heads/main"])
            .output()
            .expect("set remote HEAD")
            .status
            .success()
    );
    (repository, remote)
}

pub fn write_tracked_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture file");
}

pub fn run_cli(root: &Path, arguments: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(root)
        .args(arguments)
        .output()
        .expect("binary should run")
}

pub fn run_cli_with_path_and_vars(
    root: &Path,
    arguments: &[&str],
    command_directory: &Path,
    variables: &[(&str, &str)],
) -> Output {
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let joined = std::env::join_paths(
        std::iter::once(command_directory.to_path_buf()).chain(std::env::split_paths(&old_path)),
    )
    .expect("fake PATH");
    let mut command = Command::new(binary());
    command
        .current_dir(root)
        .args(arguments)
        .env("PATH", joined);
    for (name, value) in variables {
        command.env(name, value);
    }
    command.output().expect("binary should run")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn fake_ddev_directory(state: &Path) -> TempDir {
    let bin = tempdir().expect("fake command directory");
    let script = bin.path().join("ddev");
    let contents = format!(
        "#!/bin/sh\nset -eu\nif [ -n \"${{DDEV_FAKE_LOG:-}}\" ]; then\n  printf '%s\\n' \"$*\" >> \"$DDEV_FAKE_LOG\"\nfi\nif [ \"$1\" = \"version\" ]; then\n  printf '{{\"raw\":{{\"global-ddev-dir\":\"{global_dir}\"}}}}'\n  exit 0\nfi\nif [ \"$1\" = \"list\" ]; then\n  if [ -f \"{state}\" ]; then\n    printf '{{\"raw\":[{{\"name\":\"%s\",\"approot\":\"%s\",\"status\":\"%s\",\"mutagen_enabled\":%s,\"mutagen_status\":\"%s\"}}]}}' \"$DDEV_FAKE_NAME\" \"${{DDEV_FAKE_APPROOT:-$PWD}}\" \"${{DDEV_FAKE_STATUS:-running}}\" \"${{DDEV_FAKE_MUTAGEN_ENABLED:-false}}\" \"${{DDEV_FAKE_MUTAGEN_STATUS:-}}\"\n  else\n    printf '{{\"raw\":[]}}'\n  fi\n  exit 0\nfi\nif [ \"$1\" = \"start\" ]; then\n  : > \"{state}\"\n  exit 0\nfi\nif [ \"$1\" = \"stop\" ]; then\n  rm -f \"{state}\"\n  exit 0\nfi\nexit 0\n",
        state = state.display(),
        global_dir = state.parent().expect("fake DDEV state parent").display()
    );
    fs::write(&script, contents).expect("fake ddev script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&script)
            .expect("fake ddev metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).expect("fake ddev permissions");
    }
    bin
}

pub fn fake_pruning_ddev_directory() -> TempDir {
    let bin = tempdir().expect("fake command directory");
    let script = bin.path().join("ddev");
    fs::write(
        &script,
        r#"#!/bin/sh
set -eu
if [ "$1" = "version" ]; then
  printf '{"raw":{"global-ddev-dir":"%s"}}' "$DDEV_FAKE_GLOBAL_DIR"
  exit 0
fi
if [ "$1" = "list" ]; then
  : > "$DDEV_XDG_CONFIG_HOME/ddev/project_list.yaml"
  printf '%s\n' '{"level":"warning","msg":"The project '\''/missing/control'\'' no longer exists in the filesystem, removing it from registry"}' >&2
  printf '{"raw":[]}'
  exit 0
fi
exit 0
"#,
    )
    .expect("fake pruning ddev script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&script)
            .expect("fake pruning ddev metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).expect("fake pruning ddev permissions");
    }
    bin
}
