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
    fs::create_dir_all(directory.path().join(".ddev")).expect("DDEV directory");
    fs::write(
        directory.path().join(".ddev/config.yaml"),
        "name: fixture\ntype: php\ndocroot: public\n",
    )
    .expect("DDEV config");
    fs::create_dir_all(directory.path().join("public")).expect("public directory");
    fs::write(
        directory.path().join("public/index.php"),
        "<?php echo 'fixture';\n",
    )
    .expect("fixture entrypoint");
    fs::write(
        directory.path().join(".gitignore"),
        ".ddev-workspaces/\n.env\n",
    )
    .expect("fixture ignore file");
    commit(directory.path(), "initial fixture");
    directory
}

pub fn write_laravel_config(root: &Path, database_default: &str) {
    fs::write(
        root.join(".ddev-workspaces.toml"),
        format!(
            "version = 2\nproject_id = 'fixture'\nworktree_root = '.ddev-workspaces/worktrees'\n\n[runtime]\nadapter = 'laravel'\nhost = '.'\ndocroot = 'public'\n\n[database]\ndefault = '{database_default}'\n"
        ),
    )
    .expect("workspace configuration");
}

pub fn commit(root: &Path, message: &str) {
    assert!(run_git(root, &["add", "--all"]).status.success());
    assert!(run_git(root, &["commit", "-m", message]).status.success());
}

pub fn run_cli(root: &Path, arguments: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(root)
        .args(arguments)
        .output()
        .expect("binary should run")
}

pub fn fake_ddev_directory() -> TempDir {
    let bin = tempdir().expect("fake command directory");
    let script = bin.path().join("ddev");
    let contents = r#"#!/bin/sh
set -eu
approot=$(printenv DDEV_FAKE_APPROOT || true)
[ -n "$approot" ] || approot="$PWD"
fake_state="$approot/.ddev-workspaces"
mkdir -p "$fake_state"
databases="$fake_state/.fake-databases"
database_contents="$fake_state/.fake-database-contents"
database_ownership="$fake_state/.fake-database-ownership"
commands="$fake_state/.fake-commands"
mutagen_enabled=false
if [ -f "$fake_state/.fake-mutagen-enabled" ]; then
  mutagen_enabled=true
fi
if [ ! -f "$databases" ]; then
  printf '%s\n' db > "$databases"
fi
if [ ! -f "$database_contents" ]; then
  printf '%s\n' 'db=source-sentinel' > "$database_contents"
fi
if [ "$1" = "version" ]; then
  [ "$#" -eq 2 ] || exit 127
  [ "$2" = "--json-output" ] || exit 127
  global=$(printenv DDEV_FAKE_GLOBAL_DIR)
  printf '{"raw":{"global-ddev-dir":"%s"}}' "$global"
  exit 0
fi
if [ "$1" = "list" ]; then
  [ "$#" -eq 2 ] || exit 127
  [ "$2" = "--json-output" ] || exit 127
  [ -n "${DDEV_XDG_CONFIG_HOME:-}" ] || exit 127
  [ -d "$DDEV_XDG_CONFIG_HOME" ] || exit 127
  status=$(cat "$fake_state/.fake-status" 2>/dev/null || printf running)
  printf '{"raw":[{"name":"fixture","approot":"%s","status":"%s","httpsurl":"https://fixture.ddev.site","mutagen_enabled":%s}]}' "$approot" "$status" "$mutagen_enabled"
  exit 0
fi
if [ "$1" = "describe" ]; then
  [ "$#" -eq 2 ] || exit 127
  [ "$2" = "--json-output" ] || exit 127
  status=$(cat "$fake_state/.fake-status" 2>/dev/null || printf running)
  printf '{"raw":{"name":"fixture","approot":"%s","status":"%s","hostname":"fixture.ddev.site","hostnames":["fixture.ddev.site"],"httpsurl":"https://fixture.ddev.site","database_type":"mariadb","webserver_type":"nginx-fpm","docroot":"public","mutagen_enabled":%s}}' "$PWD" "$status" "$mutagen_enabled"
  exit 0
fi
if [ "$1" = "restart" ]; then
  [ "$#" -eq 2 ] || exit 127
  [ "$2" = "-y" ] || exit 127
  printf '%s\n' restart >> "$commands"
  if [ -f "$fake_state/.fake-restart-fails" ]; then
    exit 1
  fi
  exit 0
fi
if [ "$1" = "mutagen" ]; then
  [ "$#" -eq 2 ] || exit 127
  [ "$2" = "sync" ] || exit 127
  printf '%s\n' mutagen >> "$commands"
  exit 0
fi
if [ "$1" = "export-db" ]; then
  [ "$#" -eq 4 ] || exit 127
  case "$2" in --database=*) ;; *) exit 127 ;; esac
  [ "$3" = "--gzip" ] || exit 127
  case "$4" in --file=*) ;; *) exit 127 ;; esac
  source=$(printf '%s' "$*" | sed -n 's/.*--database=\([^ ]*\).*/\1/p')
  file=$(printf '%s' "$*" | sed -n 's/.*--file=\([^ ]*\).*/\1/p')
  if [ -f "$fake_state/.fake-export-fails" ]; then
    exit 1
  fi
  if ! grep -Fxq "$source" "$databases" 2>/dev/null; then
    exit 1
  fi
  value=$(awk -F= -v database="$source" '$1 == database { print substr($0, index($0, "=") + 1); found = 1; exit } END { if (!found) print "source-data" }' "$database_contents")
  printf '%s\n' "$value" > "$file"
  exit 0
fi
if [ "$1" = "import-db" ]; then
  [ "$#" -eq 5 ] || exit 127
  case "$2" in --database=*) ;; *) exit 127 ;; esac
  [ "$3" = "--no-drop" ] || exit 127
  [ "$4" = "--no-progress" ] || exit 127
  case "$5" in --file=*) ;; *) exit 127 ;; esac
  if [ -f "$fake_state/.fake-import-fails" ]; then
    exit 1
  fi
  destination=$(printf '%s' "$*" | sed -n 's/.*--database=\([^ ]*\).*/\1/p')
  file=$(printf '%s' "$*" | sed -n 's/.*--file=\([^ ]*\).*/\1/p')
  value=$(cat "$file")
  grep -Fxv "$destination" "$databases" > "$databases.tmp" || true
  printf '%s\n' "$destination" >> "$databases.tmp"
  mv "$databases.tmp" "$databases"
  grep -v "^$destination=" "$database_contents" > "$database_contents.tmp" || true
  printf '%s=%s\n' "$destination" "$value" >> "$database_contents.tmp"
  mv "$database_contents.tmp" "$database_contents"
  exit 0
fi
if [ "$1" = "exec" ]; then
  case "$*" in
    "exec --raw --service db -- mysql --user=root --password=root --batch --skip-column-names --execute="*) ;;
    "exec --raw --service db -- mariadb --user=root --password=root --batch --skip-column-names --execute="*) ;;
    "exec --raw --service db -- mysql --user=root --password=root --database="*" --batch --skip-column-names --execute="*) ;;
    "exec --raw --service db -- mariadb --user=root --password=root --database="*" --batch --skip-column-names --execute="*) ;;
    "exec --raw --service web -- test -e "*) ;;
    *) exit 127 ;;
  esac
  case "$*" in
    *"test -e"*)
      probe="${8:-}"
      case "$probe" in
        /var/www/html/*) probe="$approot/${probe#/var/www/html/}" ;;
        *) exit 1 ;;
      esac
      [ -e "$probe" ] || exit 1
      exit 0
      ;;
    *"CREATE TABLE IF NOT EXISTS __ddev_workspaces_ownership"*|*"DELETE FROM __ddev_workspaces_ownership"*)
      exit 0
      ;;
    *"INSERT INTO __ddev_workspaces_ownership"*)
      database=$(printf '%s' "$*" | sed -n 's/.*--database=\([^ ]*\).*/\1/p')
      token=$(printf '%s' "$*" | sed -n "s/.*VALUES ('\([^']*\)').*/\1/p")
      if [ -n "$database" ] && [ -n "$token" ]; then
        grep -v "^$database=" "$database_ownership" 2>/dev/null > "$database_ownership.tmp" || true
        printf '%s=%s\n' "$database" "$token" >> "$database_ownership.tmp"
        mv "$database_ownership.tmp" "$database_ownership"
      fi
      exit 0
      ;;
    *"SELECT token FROM __ddev_workspaces_ownership"*)
      database=$(printf '%s' "$*" | sed -n 's/.*--database=\([^ ]*\).*/\1/p')
      token=$(printf '%s' "$*" | sed -n "s/.*WHERE token = '\([^']*\)'.*/\1/p")
      if [ -n "$database" ] && [ -n "$token" ] && grep -Fxq "$database=$token" "$database_ownership" 2>/dev/null; then
        printf '%s\n' "$token"
      fi
      exit 0
      ;;
    *"INFORMATION_SCHEMA"*)
      name=$(printf '%s' "$*" | sed -n "s/.*SCHEMA_NAME = '\([^']*\)'.*/\1/p")
      if [ -n "$name" ] && grep -Fxq "$name" "$databases" 2>/dev/null; then
        printf '%s\n' "$name"
      fi
      exit 0
      ;;
    *"CREATE DATABASE"*)
      name=$(printf '%s' "$*" | sed -n 's/.*IF NOT EXISTS \([^ ]*\).*/\1/p')
      [ -n "$name" ] || name=$(printf '%s' "$*" | sed -n 's/.*CREATE DATABASE \([^ ]*\).*/\1/p')
      if [ -n "$name" ] && grep -Fxq "$name" "$databases" 2>/dev/null; then
        exit 1
      fi
      if [ -n "$name" ]; then
        printf '%s\n' "$name" >> "$databases"
        printf '%s=empty\n' "$name" >> "$database_contents"
      fi
      exit 0
      ;;
    *"DROP DATABASE"*)
      if [ -f "$fake_state/.fake-drop-fails" ]; then
        exit 1
      fi
      name=$(printf '%s' "$*" | sed -n 's/.*IF EXISTS \([^ ]*\).*/\1/p')
      if [ -n "$name" ] && [ -f "$databases" ]; then
        grep -Fxv "$name" "$databases" > "$databases.tmp" || true
        mv "$databases.tmp" "$databases"
        grep -v "^$name=" "$database_contents" > "$database_contents.tmp" || true
        mv "$database_contents.tmp" "$database_contents"
        grep -v "^$name=" "$database_ownership" 2>/dev/null > "$database_ownership.tmp" || true
        mv "$database_ownership.tmp" "$database_ownership"
      fi
      exit 0
      ;;
    *) exit 127 ;;
  esac
fi
exit 127
"#;
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

pub fn run_with_fake_ddev(root: &Path, arguments: &[&str], fake_bin: &Path) -> Output {
    run_with_fake_ddev_at(root, arguments, fake_bin, root)
}

pub fn run_with_fake_ddev_at(
    root: &Path,
    arguments: &[&str],
    fake_bin: &Path,
    app_root: &Path,
) -> Output {
    let global = tempdir().expect("fake DDEV global directory");
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let joined = std::env::join_paths(
        std::iter::once(fake_bin.to_path_buf()).chain(std::env::split_paths(&old_path)),
    )
    .expect("fake PATH");
    Command::new(binary())
        .current_dir(root)
        .args(arguments)
        .env("PATH", joined)
        .env("DDEV_FAKE_GLOBAL_DIR", global.path())
        .env("DDEV_FAKE_APPROOT", app_root)
        .output()
        .expect("binary should run")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
