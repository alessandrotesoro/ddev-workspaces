use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::command::{CommandRequest, CommandRunner, ToolError, ToolResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepository {
    pub root: PathBuf,
    pub common_dir: PathBuf,
    pub main_worktree: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseRevision {
    pub reference: String,
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRecord {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceDiagnostics {
    pub issues: Vec<String>,
    pub warnings: Vec<String>,
    pub worktrees: Vec<WorktreeRecord>,
}

impl SourceDiagnostics {
    pub fn ready(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedPath {
    path: String,
    stage: u8,
}

impl GitRepository {
    pub fn discover<R: CommandRunner>(path: &Path, runner: &mut R) -> ToolResult<Self> {
        let input = fs::canonicalize(path).map_err(|error| {
            ToolError::new(format!(
                "cannot resolve repository path {}: {error}",
                path.display()
            ))
        })?;
        let root_output = run_git(runner, &input, ["rev-parse", "--show-toplevel"])?;
        let root = fs::canonicalize(trim_one_line(&root_output)?).map_err(|error| {
            ToolError::new(format!("cannot resolve Git repository root: {error}"))
        })?;

        let common_output = run_git(runner, &root, ["rev-parse", "--git-common-dir"])?;
        let common_dir = resolve_git_path(
            &root,
            trim_one_line(&common_output)?,
            "common Git directory",
        )?;
        let main_worktree = common_dir
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ToolError::new("Git common directory has no repository parent"))?;

        Ok(Self {
            root,
            common_dir,
            main_worktree,
        })
    }

    pub fn current_head<R: CommandRunner>(&self, runner: &mut R) -> ToolResult<String> {
        let output = run_git(runner, &self.root, ["rev-parse", "HEAD"])?;
        let sha = trim_one_line(&output)?.to_owned();
        if !is_full_commit_id(&sha) {
            return Err(ToolError::new("Git returned an invalid HEAD commit"));
        }
        Ok(sha)
    }

    pub fn resolve_base<R: CommandRunner>(
        &self,
        requested: Option<&str>,
        runner: &mut R,
    ) -> ToolResult<BaseRevision> {
        match requested {
            Some(reference) => {
                let sha = self.resolve_local_commit(reference, runner)?;
                Ok(BaseRevision {
                    reference: reference.to_owned(),
                    sha,
                })
            }
            None => self.resolve_remote_default(runner),
        }
    }

    pub fn resolve_local_commit<R: CommandRunner>(
        &self,
        reference: &str,
        runner: &mut R,
    ) -> ToolResult<String> {
        if reference.trim().is_empty() || reference.starts_with('-') {
            return Err(ToolError::new(format!(
                "base revision `{reference}` is invalid or ambiguous; provide one local commit"
            )));
        }
        let revision = format!("{reference}^{{commit}}");
        let request = git_request(
            &self.root,
            [
                "rev-parse".to_owned(),
                "--verify".to_owned(),
                "--quiet".to_owned(),
                "--end-of-options".to_owned(),
                revision,
            ],
        );
        let output = runner.run(&request)?;
        if !output.success() {
            return Err(ToolError::new(format!(
                "base revision `{reference}` is not one locally resolvable commit; run `git fetch origin <branch>` manually and retry"
            )));
        }
        let sha = trim_one_line(&output.stdout)?.to_owned();
        if !is_full_commit_id(&sha) {
            return Err(ToolError::new(format!(
                "base revision `{reference}` did not resolve to a full local commit"
            )));
        }
        Ok(sha)
    }

    fn resolve_remote_default<R: CommandRunner>(&self, runner: &mut R) -> ToolResult<BaseRevision> {
        let request = git_request(
            &self.root,
            [
                "ls-remote".to_owned(),
                "--symref".to_owned(),
                "origin".to_owned(),
                "HEAD".to_owned(),
            ],
        );
        let output = runner.run(&request)?;
        if !output.success() {
            return Err(ToolError::new(
                "could not query origin's advertised HEAD; run `git fetch origin <default-branch>` manually and retry",
            ));
        }
        let (references, heads) = parse_remote_head(&output.stdout);
        if references.len() != 1 || heads.len() != 1 || references[0] != heads[0].0 {
            return Err(ToolError::new(
                "origin's advertised HEAD is missing or ambiguous; inspect `git ls-remote --symref origin HEAD` manually",
            ));
        }
        let reference = references[0].clone();
        let sha = heads[0].1.clone();
        self.require_local_commit(&sha, runner)?;
        Ok(BaseRevision { reference, sha })
    }

    fn require_local_commit<R: CommandRunner>(&self, sha: &str, runner: &mut R) -> ToolResult<()> {
        let request = git_request(
            &self.root,
            [
                "cat-file".to_owned(),
                "-e".to_owned(),
                format!("{sha}^{{commit}}"),
            ],
        );
        let output = runner.run(&request)?;
        if output.success() {
            Ok(())
        } else {
            Err(ToolError::new(format!(
                "origin's advertised commit {sha} is not present locally; run `git fetch origin <default-branch>` manually and retry"
            )))
        }
    }

    pub fn branch_exists<R: CommandRunner>(
        &self,
        branch: &str,
        runner: &mut R,
    ) -> ToolResult<bool> {
        let request = git_request(
            &self.root,
            [
                "show-ref".to_owned(),
                "--verify".to_owned(),
                "--quiet".to_owned(),
                format!("refs/heads/{branch}"),
            ],
        );
        let output = runner.run(&request)?;
        match output.status {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(ToolError::new(
                "Git could not inspect whether the requested branch exists; refusing workspace creation",
            )),
        }
    }

    pub fn worktrees<R: CommandRunner>(&self, runner: &mut R) -> ToolResult<Vec<WorktreeRecord>> {
        let output = run_git(
            runner,
            &self.root,
            ["worktree", "list", "--porcelain", "-z"],
        )?;
        Ok(parse_worktrees(&output))
    }

    pub fn ensure_worktree_available<R: CommandRunner>(
        &self,
        path: &Path,
        branch: &str,
        runner: &mut R,
    ) -> ToolResult<()> {
        match fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(ToolError::new(format!(
                    "workspace path {} already exists; choose another name or remove it manually",
                    path.display()
                )));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(ToolError::new(format!(
                    "workspace path {} cannot be inspected: {error}",
                    path.display()
                )));
            }
            Err(_) => {}
        }
        for worktree in self.worktrees(runner)? {
            if worktree.path == path {
                return Err(ToolError::new(format!(
                    "workspace path {} is already registered by Git",
                    path.display()
                )));
            }
            if worktree.branch.as_deref() == Some(&format!("refs/heads/{branch}")) {
                return Err(ToolError::new(format!(
                    "branch `{branch}` is already used by a Git worktree at {}",
                    worktree.path.display()
                )));
            }
        }
        if self.branch_exists(branch, runner)? {
            return Err(ToolError::new(format!(
                "branch `{branch}` already exists; v1 never adopts or resets an existing branch"
            )));
        }
        Ok(())
    }

    pub fn add_worktree<R: CommandRunner>(
        &self,
        path: &Path,
        branch: &str,
        base_sha: &str,
        runner: &mut R,
    ) -> ToolResult<()> {
        let request = git_request(
            &self.root,
            [
                "worktree".to_owned(),
                "add".to_owned(),
                "-b".to_owned(),
                branch.to_owned(),
                path.display().to_string(),
                base_sha.to_owned(),
            ],
        )
        .mutating();
        let output = runner.run(&request)?;
        if output.success() {
            Ok(())
        } else {
            Err(ToolError::new(format!(
                "Git could not create workspace {} from {base_sha}; preserve the ownership record and inspect Git manually",
                path.display()
            )))
        }
    }

    pub fn require_stage_zero_file<R: CommandRunner>(
        &self,
        path: &Path,
        runner: &mut R,
    ) -> ToolResult<()> {
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            ToolError::new(format!(
                "declared template {} is outside the Git worktree",
                path.display()
            ))
        })?;
        let relative = relative.to_str().ok_or_else(|| {
            ToolError::new(format!(
                "declared template {} cannot be represented as a Git path",
                path.display()
            ))
        })?;
        let output = run_git(
            runner,
            &self.root,
            [
                "ls-files".to_owned(),
                "--stage".to_owned(),
                "-z".to_owned(),
                "--".to_owned(),
                relative.to_owned(),
            ],
        )?;
        let tracked = parse_tracked_paths(&output);
        if tracked
            .iter()
            .any(|entry| entry.stage == 0 && entry.path == relative)
        {
            return Ok(());
        }
        Err(ToolError::new(format!(
            "declared template {} must be a stage-zero tracked Git file; untracked templates are not accepted",
            path.display()
        )))
    }

    pub fn remove_worktree<R: CommandRunner>(&self, path: &Path, runner: &mut R) -> ToolResult<()> {
        let request = git_request(
            &self.root,
            [
                "worktree".to_owned(),
                "remove".to_owned(),
                path.display().to_string(),
            ],
        )
        .mutating();
        let output = runner.run(&request)?;
        if output.success() {
            Ok(())
        } else {
            Err(ToolError::new(format!(
                "Git refused to remove {} without force; resolve the reported cleanliness, lock, or submodule issue manually",
                path.display()
            )))
        }
    }

    pub fn diagnose<R: CommandRunner>(
        &self,
        expected_commit: Option<&str>,
        runner: &mut R,
    ) -> ToolResult<SourceDiagnostics> {
        let mut diagnostics = SourceDiagnostics {
            worktrees: self.worktrees(runner)?,
            ..SourceDiagnostics::default()
        };
        diagnostics.warnings.extend(
            diagnostics
                .worktrees
                .iter()
                .filter(|worktree| worktree.locked)
                .map(|worktree| {
                    format!("locked Git worktree metadata: {}", worktree.path.display())
                }),
        );
        diagnostics.warnings.extend(
            diagnostics
                .worktrees
                .iter()
                .filter(|worktree| worktree.head.is_some() && worktree.branch.is_none())
                .map(|worktree| {
                    format!(
                        "detached Git worktree metadata: {}",
                        worktree.path.display()
                    )
                }),
        );
        let prunable = diagnostics
            .worktrees
            .iter()
            .filter(|worktree| worktree.prunable)
            .count();
        if prunable > 0 {
            diagnostics.warnings.push(format!(
                "{prunable} prunable Git worktree record(s); run a manual `git worktree prune --dry-run` to inspect"
            ));
        }

        let sparse = run_git_allow_failure(
            runner,
            &self.root,
            ["config", "--bool", "core.sparseCheckout"],
        )?;
        if sparse.success() && sparse.stdout.trim() == "true" {
            diagnostics.issues.push(
                "sparse checkout is enabled; restore the complete working tree manually".to_owned(),
            );
        }

        let flags = run_git(runner, &self.root, ["ls-files", "-v", "-z"])?;
        for (flag, path) in parse_prefixed_paths(&flags) {
            if flag == 'S' || flag.is_ascii_lowercase() {
                diagnostics.issues.push(format!(
                    "hidden index flag `{flag}`: {path} (clear it manually with `git update-index --no-skip-worktree --no-assume-unchanged -- <path>` )"
                ));
            }
        }

        let tracked =
            parse_tracked_paths(&run_git(runner, &self.root, ["ls-files", "--stage", "-z"])?);
        let tracked_paths: Vec<String> = tracked
            .iter()
            .filter(|entry| entry.stage == 0)
            .map(|entry| entry.path.clone())
            .collect();
        for entry in tracked.iter().filter(|entry| entry.stage == 0) {
            let path = self.root.join(&entry.path);
            if fs::symlink_metadata(&path).is_err() {
                diagnostics
                    .issues
                    .push(format!("tracked path missing: {}", entry.path));
            }
        }

        let current_head = self.current_head(runner)?;
        if let Some(expected_commit) = expected_commit {
            if current_head != expected_commit {
                diagnostics.issues.push(format!(
                    "worktree HEAD {current_head} differs from expected base {expected_commit}"
                ));
            }
        }
        let comparison = expected_commit.unwrap_or("HEAD");
        let index_diff = run_git_allow_failure(
            runner,
            &self.root,
            ["diff-index", "--quiet", comparison, "--"],
        )?;
        if !index_diff.success() {
            diagnostics
                .issues
                .push(format!("tracked content or mode differs from {comparison}"));
        }
        let worktree_diff =
            run_git_allow_failure(runner, &self.root, ["diff-files", "--quiet", "--"])?;
        if !worktree_diff.success() {
            diagnostics
                .issues
                .push("working-tree files differ from the index".to_owned());
        }

        let branch = run_git_allow_failure(
            runner,
            &self.root,
            ["symbolic-ref", "--quiet", "--short", "HEAD"],
        )?;
        if !branch.success() {
            diagnostics.issues.push("repository is detached".to_owned());
        }

        let lfs_paths = self.lfs_paths(&tracked_paths, runner)?;
        if !lfs_paths.is_empty() {
            let lfs = run_git_allow_failure(runner, &self.root, ["lfs", "version"])?;
            if !lfs.success() {
                diagnostics.issues.push(format!(
                    "Git LFS is required for {} tracked path(s), but `git lfs` is unavailable; install Git LFS and rerun doctor",
                    lfs_paths.len()
                ));
            } else {
                for path in lfs_paths {
                    let full_path = self.root.join(&path);
                    if is_lfs_pointer(&full_path) {
                        diagnostics.issues.push(format!(
                            "LFS content is pointer-only: {path}; materialize it with a manual local/credentialed Git LFS operation"
                        ));
                    }
                }
            }
        }

        let submodules = self.submodule_status(runner)?;
        diagnostics.issues.extend(submodules);
        Ok(diagnostics)
    }

    pub fn initialize_submodules<R: CommandRunner>(&self, runner: &mut R) -> ToolResult<()> {
        let request = git_request(
            &self.root,
            [
                "submodule".to_owned(),
                "update".to_owned(),
                "--init".to_owned(),
                "--recursive".to_owned(),
            ],
        )
        .mutating();
        let output = runner.run(&request)?;
        if !output.success() {
            let issues = self.submodule_status(runner)?;
            let detail = if issues.is_empty() {
                "inspect the named submodule and authenticate its private repository manually"
                    .to_owned()
            } else {
                format!(
                    "{}; authenticate its private repository manually",
                    issues.join(", ")
                )
            };
            return Err(ToolError::new(format!(
                "recursive submodule initialization failed: {detail}"
            )));
        }
        let remaining = self.submodule_status(runner)?;
        if remaining.is_empty() {
            Ok(())
        } else {
            Err(ToolError::new(format!(
                "recursive submodule verification failed: {}; inspect those paths manually",
                remaining.join(", ")
            )))
        }
    }

    pub fn checkout_lfs<R: CommandRunner>(&self, runner: &mut R) -> ToolResult<()> {
        let tracked =
            parse_tracked_paths(&run_git(runner, &self.root, ["ls-files", "--stage", "-z"])?);
        let paths = tracked
            .iter()
            .filter(|entry| entry.stage == 0)
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        if self.lfs_paths(&paths, runner)?.is_empty() {
            return Ok(());
        }
        let version = run_git_allow_failure(runner, &self.root, ["lfs", "version"])?;
        if !version.success() {
            return Err(ToolError::new(
                "Git LFS is required by tracked attributes but is unavailable; install Git LFS and rerun the source preparation",
            ));
        }
        let request = git_request(&self.root, ["lfs".to_owned(), "checkout".to_owned()]).mutating();
        let output = runner.run(&request)?;
        if output.success() {
            Ok(())
        } else {
            Err(ToolError::new(
                "Git LFS could not materialize local content; run a credentialed local Git LFS operation manually",
            ))
        }
    }

    fn submodule_status<R: CommandRunner>(&self, runner: &mut R) -> ToolResult<Vec<String>> {
        let output =
            run_git_allow_failure(runner, &self.root, ["submodule", "status", "--recursive"])?;
        if !output.success() {
            let mut issues = parse_submodule_issues(&output.stdout);
            if issues.is_empty() {
                issues.push(
                    "submodule status could not be read; inspect nested gitlinks manually"
                        .to_owned(),
                );
            } else {
                issues.insert(0, "submodule status command failed".to_owned());
            }
            return Ok(issues);
        }
        Ok(parse_submodule_issues(&output.stdout))
    }

    fn lfs_paths<R: CommandRunner>(
        &self,
        tracked_paths: &[String],
        runner: &mut R,
    ) -> ToolResult<Vec<String>> {
        if tracked_paths.is_empty() {
            return Ok(Vec::new());
        }
        const MAX_BATCH_BYTES: usize = 32 * 1024;
        let mut lfs_paths = Vec::new();
        let mut batch = Vec::new();
        let mut batch_bytes = 0usize;
        for path in tracked_paths {
            let path_bytes = path.len().saturating_add(1);
            if !batch.is_empty() && batch_bytes.saturating_add(path_bytes) > MAX_BATCH_BYTES {
                lfs_paths.extend(self.lfs_paths_batch(&batch, runner)?);
                batch.clear();
                batch_bytes = 0;
            }
            batch.push(path.as_str());
            batch_bytes = batch_bytes.saturating_add(path_bytes);
        }
        if !batch.is_empty() {
            lfs_paths.extend(self.lfs_paths_batch(&batch, runner)?);
        }
        Ok(lfs_paths)
    }

    fn lfs_paths_batch<R: CommandRunner>(
        &self,
        tracked_paths: &[&str],
        runner: &mut R,
    ) -> ToolResult<Vec<String>> {
        let mut args = vec![
            "check-attr".to_owned(),
            "-z".to_owned(),
            "filter".to_owned(),
            "--".to_owned(),
        ];
        args.extend(tracked_paths.iter().map(|path| (*path).to_owned()));
        let output = run_git_allow_failure(runner, &self.root, args)?;
        if !output.success() {
            return Err(ToolError::new(
                "Git attribute inspection failed; refusing to infer LFS readiness",
            ));
        }
        let mut fields = output.stdout.split('\0').collect::<Vec<_>>();
        if fields.last() == Some(&"") {
            fields.pop();
        }
        if fields.len() % 3 != 0 {
            return Err(ToolError::new(
                "Git returned malformed attribute output; refusing to infer LFS readiness",
            ));
        }
        Ok(fields
            .chunks_exact(3)
            .filter(|fields| fields[1] == "filter" && fields[2] == "lfs")
            .map(|fields| fields[0].to_owned())
            .collect())
    }

    pub fn worktree_is_clean<R: CommandRunner>(
        &self,
        worktree: &Path,
        runner: &mut R,
    ) -> ToolResult<()> {
        let status = run_git_allow_failure(
            runner,
            worktree,
            ["status", "--porcelain", "--untracked-files=normal"],
        )?;
        if !status.success() {
            return Err(ToolError::new(format!(
                "could not inspect worktree {}; refusing removal",
                worktree.display()
            )));
        }
        if !status.stdout.trim().is_empty() {
            return Err(ToolError::new(format!(
                "worktree {} is dirty or has untracked files; commit or remove them manually",
                worktree.display()
            )));
        }
        let submodule_status =
            run_git_allow_failure(runner, worktree, ["submodule", "status", "--recursive"])?;
        if !submodule_status.success() {
            return Err(ToolError::new(format!(
                "could not inspect submodules in {}; refusing removal",
                worktree.display()
            )));
        }
        let submodules = parse_submodule_issues(&submodule_status.stdout);
        if !submodules.is_empty() {
            return Err(ToolError::new(format!(
                "worktree {} has unsafe submodule state: {}",
                worktree.display(),
                submodules.join(", ")
            )));
        }
        Ok(())
    }
}

pub fn git_request<I, S>(cwd: &Path, args: I) -> CommandRequest
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    CommandRequest::new("git", args).cwd(cwd)
}

fn run_git<R: CommandRunner, I, S>(runner: &mut R, cwd: &Path, args: I) -> ToolResult<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let request = CommandRequest::new("git", args).cwd(cwd);
    let output = runner.run(&request)?;
    if output.success() {
        Ok(output.stdout)
    } else {
        Err(ToolError::new(format!(
            "Git command `{}` failed; inspect Git's diagnostic and retry manually",
            request.display()
        )))
    }
}

fn run_git_allow_failure<R: CommandRunner, I, S>(
    runner: &mut R,
    cwd: &Path,
    args: I,
) -> ToolResult<crate::command::CommandOutput>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let request = CommandRequest::new("git", args).cwd(cwd);
    runner.run(&request)
}

fn resolve_git_path(root: &Path, value: &str, label: &str) -> ToolResult<PathBuf> {
    let path = Path::new(value);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    fs::canonicalize(&path).map_err(|error| {
        ToolError::new(format!(
            "cannot resolve {label} {}: {error}",
            path.display()
        ))
    })
}

fn trim_one_line(value: &str) -> ToolResult<&str> {
    let value = value.trim();
    if value.is_empty() {
        Err(ToolError::new(
            "Git returned an empty machine-readable value",
        ))
    } else {
        Ok(value)
    }
}

pub fn is_full_commit_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_remote_head(output: &str) -> (Vec<String>, Vec<(String, String)>) {
    let mut references = Vec::new();
    let mut heads = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        if let Some(reference) = line.strip_prefix("ref: ") {
            let mut fields = reference.split_whitespace();
            if let (Some(reference), Some(_)) = (fields.next(), fields.next()) {
                references.push(reference.to_owned());
            }
        } else {
            let mut fields = line.split_whitespace();
            if let (Some(sha), Some(name)) = (fields.next(), fields.next()) {
                if name == "HEAD" && is_full_commit_id(sha) {
                    heads.push((
                        references.last().cloned().unwrap_or_default(),
                        sha.to_owned(),
                    ));
                }
            }
        }
    }
    (references, heads)
}

fn parse_worktrees(output: &str) -> Vec<WorktreeRecord> {
    let mut records = Vec::new();
    let mut current: Option<WorktreeRecord> = None;
    for field in output.split('\0') {
        if field.is_empty() {
            continue;
        }
        if let Some(path) = field.strip_prefix("worktree ") {
            if let Some(record) = current.take() {
                records.push(record);
            }
            current = Some(WorktreeRecord {
                path: PathBuf::from(path),
                head: None,
                branch: None,
                locked: false,
                prunable: false,
            });
        } else if let Some(record) = current.as_mut() {
            if let Some(head) = field.strip_prefix("HEAD ") {
                record.head = Some(head.to_owned());
            } else if let Some(branch) = field.strip_prefix("branch ") {
                record.branch = Some(branch.to_owned());
            } else if field == "locked" || field.starts_with("locked ") {
                record.locked = true;
            } else if field == "prunable" || field.starts_with("prunable ") {
                record.prunable = true;
            }
        }
    }
    if let Some(record) = current {
        records.push(record);
    }
    records
}

fn parse_prefixed_paths(output: &str) -> Vec<(char, String)> {
    output
        .split('\0')
        .filter_map(|entry| {
            if entry.is_empty() {
                return None;
            }
            let mut chars = entry.chars();
            let flag = chars.next()?;
            let path = chars.as_str().trim_start_matches([' ', '\t']).to_owned();
            (!path.is_empty()).then_some((flag, path))
        })
        .collect()
}

fn parse_tracked_paths(output: &str) -> Vec<TrackedPath> {
    output
        .split('\0')
        .filter_map(|entry| {
            let (metadata, path) = entry.split_once('\t')?;
            let fields = metadata.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 3 {
                return None;
            }
            let stage = fields[2].parse::<u8>().ok()?;
            Some(TrackedPath {
                path: path.to_owned(),
                stage,
            })
        })
        .collect()
}

fn parse_submodule_issues(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| line.chars().next().is_some_and(|marker| marker != ' '))
        .map(|line| {
            let marker = line.chars().next().unwrap_or('?');
            let path = line
                .get(1..)
                .unwrap_or_default()
                .trim_start()
                .split_once(char::is_whitespace)
                .map(|(_, path)| path.trim())
                .filter(|path| !path.is_empty())
                .unwrap_or("unknown-submodule");
            let reason = match marker {
                '-' => "uninitialized",
                '+' => "checked-out commit differs from the recorded gitlink",
                'U' => "conflicted",
                _ => "reported by Git",
            };
            format!("submodule {path}: {reason}")
        })
        .collect()
}

fn is_lfs_pointer(path: &Path) -> bool {
    const POINTER_PREFIX: &[u8] = b"version https://git-lfs.github.com/spec/v1\n";
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut prefix = vec![0; POINTER_PREFIX.len()];
    let Ok(read) = file.read(&mut prefix) else {
        return false;
    };
    read == POINTER_PREFIX.len() && prefix == POINTER_PREFIX
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandOutput, CommandRunner};

    #[derive(Default)]
    struct FakeRunner {
        responses: Vec<CommandOutput>,
    }

    impl CommandRunner for FakeRunner {
        fn run(&mut self, _request: &CommandRequest) -> ToolResult<CommandOutput> {
            Ok(self.responses.pop().unwrap_or_else(|| CommandOutput {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            }))
        }
    }

    #[test]
    fn remote_head_parser_requires_one_symbolic_head_and_sha() {
        let (refs, heads) = parse_remote_head(
            "ref: refs/heads/main\tHEAD\n0123456789012345678901234567890123456789\tHEAD\n",
        );

        assert_eq!(refs, vec!["refs/heads/main"]);
        assert_eq!(
            heads,
            vec![(
                "refs/heads/main".to_owned(),
                "0123456789012345678901234567890123456789".to_owned()
            )]
        );
    }

    #[test]
    fn prefixed_index_paths_expose_hidden_flags() {
        let paths = parse_prefixed_paths("H normal.txt\0S hidden.txt\0h assumed.txt\0");

        assert_eq!(paths[1], ('S', "hidden.txt".to_owned()));
        assert_eq!(paths[2], ('h', "assumed.txt".to_owned()));
    }

    #[test]
    fn worktree_parser_keeps_locked_and_prunable_markers() {
        let records = parse_worktrees(
            "worktree /tmp/main\0HEAD abc\0branch refs/heads/main\0\0worktree /tmp/old\0HEAD def\0prunable gone\0",
        );

        assert_eq!(records.len(), 2);
        assert!(records[1].prunable);
    }

    #[test]
    fn submodule_parser_names_uninitialized_path() {
        let issues =
            parse_submodule_issues("-0123456789012345678901234567890123456789 vendor/private\n");

        assert_eq!(issues, vec!["submodule vendor/private: uninitialized"]);
    }

    #[test]
    fn template_stage_check_rejects_untracked_paths() {
        let mut runner = FakeRunner {
            responses: vec![CommandOutput {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            }],
        };
        let repository = GitRepository {
            root: PathBuf::from("/tmp/repo"),
            common_dir: PathBuf::from("/tmp/repo/.git"),
            main_worktree: PathBuf::from("/tmp/repo"),
        };

        let error = repository
            .require_stage_zero_file(&repository.root.join(".env.example"), &mut runner)
            .unwrap_err();

        assert!(error.to_string().contains("stage-zero tracked Git file"));
    }

    #[test]
    fn failed_remote_query_is_sanitized() {
        let mut runner = FakeRunner {
            responses: vec![CommandOutput {
                status: 128,
                stdout: String::new(),
                stderr: "ssh://secret@example.invalid/private".to_owned(),
            }],
        };
        let repository = GitRepository {
            root: PathBuf::from("/tmp/repo"),
            common_dir: PathBuf::from("/tmp/repo/.git"),
            main_worktree: PathBuf::from("/tmp/repo"),
        };

        let error = repository.resolve_base(None, &mut runner).unwrap_err();

        assert!(!error.to_string().contains("secret@example"));
        assert!(error.to_string().contains("git fetch"));
    }
}
