use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::ArgMatches;

use crate::command::{CommandRunner, RealCommandRunner, ToolError, ToolResult};
use crate::config::{self, CheckRule, CommandRule, ProjectConfig};
use crate::ddev::{self, DdevProject};
use crate::git::{self, GitRepository, SourceDiagnostics};
use crate::state::{self, OwnershipRecord, RecordEntry};

pub fn run(matches: ArgMatches) -> ToolResult<u8> {
    let Some((command, arguments)) = matches.subcommand() else {
        return Err(ToolError::usage("one command is required"));
    };
    match command {
        "doctor" => doctor(arguments),
        "list" => list(arguments),
        "create" => create(arguments),
        "remove" => remove(arguments),
        _ => Err(ToolError::usage(format!("unsupported command `{command}`"))),
    }
}

fn doctor(arguments: &ArgMatches) -> ToolResult<u8> {
    let path = arguments
        .get_one::<String>("path")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut runner = RealCommandRunner::new(false);
    let repository = GitRepository::discover(&path, &mut runner)?;
    let diagnostics = repository.diagnose(None, &mut runner)?;

    println!("Repository: {}", repository.root.display());
    println!("Git common directory: {}", repository.common_dir.display());
    print_source_diagnostics(&diagnostics);
    let (managed_record, ownership_issues) = ownership_state_for_path(&repository);
    for issue in &ownership_issues {
        println!("Ownership: NOT READY — {issue}");
    }

    let config_result = ProjectConfig::load(&repository.main_worktree).and_then(|config| {
        config.validate(&repository.main_worktree, &mut runner)?;
        Ok(config)
    });
    let config_ready = match config_result {
        Ok(config) => {
            println!("Configuration: READY");
            let mut ready = true;
            if let Some((record_name, record)) = &managed_record {
                if let Err(error) =
                    verify_removal_target(&repository, &config, record_name, record, &mut runner)
                {
                    println!("Ownership: NOT READY — {error}");
                    ready = false;
                }
                if let Some(reason) = runtime_precondition_failure(&config, &repository.root) {
                    println!("Runtime: NOT READY — {reason}");
                    ready = false;
                } else {
                    println!("Runtime: READY");
                }
                println!("Ownership: {}", record.ddev_name);
            }
            if let Some(ddev_config) = &config.ddev {
                let app_root = config::safe_join(&repository.root, &ddev_config.app_root)?;
                match ddev::list(&mut runner, &app_root) {
                    Ok(inspection) => {
                        for warning in &inspection.warnings {
                            println!("DDEV: {warning}");
                        }
                        if let Some((_, record)) = &managed_record {
                            match find_owned_ddev(&inspection.entries, &record.ddev_name, &app_root)
                            {
                                Ok(Some(project)) => {
                                    match ddev::require_ready_identity(
                                        &inspection,
                                        &record.ddev_name,
                                        &app_root,
                                    ) {
                                        Ok(_) => println!(
                                            "DDEV: {} @ {} (running, {})",
                                            project.name,
                                            project.approot,
                                            if project.mutagen_enabled {
                                                "Mutagen ok"
                                            } else {
                                                "Mutagen disabled"
                                            }
                                        ),
                                        Err(error) => {
                                            println!("DDEV: NOT READY — {error}");
                                            ready = false;
                                        }
                                    }
                                }
                                Ok(None) => {
                                    println!(
                                        "DDEV: NOT READY — managed identity `{}` is not registered",
                                        record.ddev_name
                                    );
                                    ready = false;
                                }
                                Err(error) => {
                                    println!("DDEV: NOT READY — {error}");
                                    ready = false;
                                }
                            }
                        } else {
                            println!("DDEV: READY to inspect");
                        }
                    }
                    Err(error) => {
                        println!("DDEV: NOT READY — {error}");
                        ready = false;
                    }
                }
            } else {
                println!("DDEV: skipped (not configured)");
            }
            ready
        }
        Err(error) => {
            println!("Configuration: NOT READY — {error}");
            false
        }
    };

    if diagnostics.ready() && config_ready && ownership_issues.is_empty() {
        println!("READY");
        Ok(0)
    } else {
        println!(
            "NOT READY — restore the reported repository or configuration state, then rerun doctor"
        );
        Ok(1)
    }
}

fn ownership_state_for_path(
    repository: &GitRepository,
) -> (Option<(String, OwnershipRecord)>, Vec<String>) {
    let entries = match state::list(&repository.common_dir) {
        Ok(entries) => entries,
        Err(error) => return (None, vec![error.to_string()]),
    };
    let mut issues = Vec::new();
    let mut managed_record = None;
    for entry in entries {
        match entry.record {
            Ok(record) => {
                if Path::new(&record.common_directory) != repository.common_dir {
                    issues.push(format!(
                        "record `{}` points to another Git common directory",
                        entry.name
                    ));
                } else if Path::new(&record.worktree_path) == repository.root {
                    managed_record = Some((entry.name, record));
                }
            }
            Err(error) => issues.push(format!("record `{}` is invalid: {error}", entry.name)),
        }
    }
    (managed_record, issues)
}

fn list(_arguments: &ArgMatches) -> ToolResult<u8> {
    let mut runner = RealCommandRunner::new(false);
    let repository = GitRepository::discover(Path::new("."), &mut runner)?;
    let manager_root = &repository.main_worktree;
    let entries = state::list(&repository.common_dir)?;
    if entries.is_empty() {
        println!("No managed workspaces for {}", manager_root.display());
        println!("READY — list complete");
        return Ok(0);
    }

    let config_result = ProjectConfig::load(manager_root).and_then(|config| {
        config.validate(manager_root, &mut runner)?;
        Ok(config)
    });
    let config = config_result.as_ref().ok();
    let config_error = config_result
        .as_ref()
        .err()
        .filter(|error| !error.to_string().starts_with("configuration is required"));
    let ddev_inspection = config
        .filter(|config| config.ddev.is_some())
        .map(|_| ddev::list(&mut runner, manager_root));
    if let Some(Ok(inspection)) = &ddev_inspection {
        for warning in &inspection.warnings {
            println!("DDEV: {warning}");
        }
    }
    let mut all_ready = true;
    for entry in entries {
        all_ready &= print_list_entry(
            &repository,
            config,
            config_error,
            ddev_inspection.as_ref(),
            &entry,
            &mut runner,
        );
    }
    if all_ready {
        println!("READY — list complete");
        Ok(0)
    } else {
        println!("NOT READY — one or more managed workspaces require attention");
        Ok(1)
    }
}

fn print_list_entry<R: CommandRunner>(
    repository: &GitRepository,
    config: Option<&ProjectConfig>,
    config_error: Option<&ToolError>,
    ddev_inspection: Option<&Result<ddev::DdevInspection, ToolError>>,
    entry: &RecordEntry,
    runner: &mut R,
) -> bool {
    let record = match &entry.record {
        Ok(record) => record,
        Err(error) => {
            println!("{}: INVALID RECORD — {error}", entry.name);
            return false;
        }
    };
    if Path::new(&record.common_directory) != repository.common_dir {
        println!(
            "{}: NOT READY — ownership record points to another Git common directory",
            entry.name
        );
        return false;
    }
    if record.branch != entry.name {
        println!(
            "{}: INVALID RECORD — branch does not match the record filename",
            entry.name
        );
        return false;
    }
    if !Path::new(&record.worktree_path).is_absolute() {
        println!(
            "{}: INVALID RECORD — worktree path must be absolute",
            entry.name
        );
        return false;
    }
    if !git::is_full_commit_id(&record.base_sha) {
        println!(
            "{}: INVALID RECORD — base_sha is not a full commit ID",
            entry.name
        );
        return false;
    }
    if let Some(error) = config_error {
        println!(
            "{}: NOT READY — current configuration is invalid: {error}",
            entry.name
        );
        return false;
    }
    let path = PathBuf::from(&record.worktree_path);
    if let Some(config) = config {
        if record.project_id != config.project_id {
            println!(
                "{}: INVALID RECORD — project ID does not match current configuration",
                entry.name
            );
            return false;
        }
        let expected_root =
            match config::safe_join(&repository.main_worktree, &config.workspace_root) {
                Ok(expected_root) => expected_root,
                Err(error) => {
                    println!("{}: NOT READY — {error}", entry.name);
                    return false;
                }
            };
        if path != expected_root.join(&entry.name) {
            println!(
                "{}: NOT READY — ownership record path does not match configured workspace path",
                entry.name
            );
            return false;
        }
    }
    if !path.exists() {
        println!("{}: MISSING PATH — {}", entry.name, path.display());
        return false;
    }
    let Ok(canonical) = fs::canonicalize(&path) else {
        println!(
            "{}: NOT READY — managed worktree path could not be canonicalized",
            entry.name
        );
        return false;
    };
    if canonical != path {
        println!(
            "{}: NOT READY — managed worktree path is symlinked or otherwise non-canonical",
            entry.name
        );
        return false;
    }
    let worktree_repository = match GitRepository::discover(&path, runner) {
        Ok(repository) => repository,
        Err(error) => {
            println!("{}: NOT READY — {error}", entry.name);
            return false;
        }
    };
    if worktree_repository.common_dir != repository.common_dir {
        println!(
            "{}: NOT READY — managed path belongs to another Git common directory",
            entry.name
        );
        return false;
    }
    let source = match worktree_repository.diagnose(None, runner) {
        Ok(source) => source,
        Err(error) => {
            println!("{}: NOT READY — {error}", entry.name);
            return false;
        }
    };
    let Some(worktree) = source
        .worktrees
        .iter()
        .find(|worktree| worktree.path == path)
    else {
        println!(
            "{}: NOT READY — Git does not currently prove the recorded worktree path",
            entry.name
        );
        return false;
    };
    if worktree.branch.as_deref() != Some(&format!("refs/heads/{}", entry.name)) {
        println!(
            "{}: NOT READY — Git worktree branch does not match the ownership record",
            entry.name
        );
        return false;
    }
    if !source.ready() {
        println!("{}: NOT READY — {}", entry.name, source.issues.join("; "));
        return false;
    }
    if let Err(error) = worktree_repository.worktree_is_clean(&path, runner) {
        println!("{}: NOT READY — {error}", entry.name);
        return false;
    }
    let Some(config) = config else {
        println!(
            "{}: SOURCE-ONLY — configuration is missing from the manager root",
            entry.name
        );
        return true;
    };
    let expected_name = match ddev::expected_name(&config.project_id, &entry.name) {
        Ok(expected_name) => expected_name,
        Err(error) => {
            println!("{}: NOT READY — {error}", entry.name);
            return false;
        }
    };
    if record.ddev_name != expected_name {
        println!(
            "{}: INVALID RECORD — DDEV name does not match the deterministic project identity",
            entry.name
        );
        return false;
    }
    if let Some(reason) = runtime_precondition_failure(config, &path) {
        println!("{}: SOURCE-ONLY — {reason}", entry.name);
        return true;
    }
    let Some(ddev_config) = &config.ddev else {
        println!(
            "{}: READY — source and configured non-DDEV checks pass",
            entry.name
        );
        return true;
    };
    let app_root = match config::safe_join(&path, &ddev_config.app_root) {
        Ok(app_root) => app_root,
        Err(error) => {
            println!("{}: NOT READY — {error}", entry.name);
            return false;
        }
    };
    let inspection = match ddev_inspection {
        Some(Ok(inspection)) => inspection,
        Some(Err(error)) => {
            println!("{}: NOT READY — DDEV list failed: {error}", entry.name);
            return false;
        }
        None => {
            println!(
                "{}: NOT READY — DDEV inspection was not prepared",
                entry.name
            );
            return false;
        }
    };
    match find_owned_ddev(&inspection.entries, &record.ddev_name, &app_root) {
        Ok(Some(project))
            if project.status.eq_ignore_ascii_case("running") && ddev_mutagen_ready(&project) =>
        {
            println!("{}: READY — running at {}", entry.name, path.display());
            true
        }
        Ok(Some(_)) => {
            println!(
                "{}: NOT READY — DDEV identity is stopped or Mutagen is unhealthy",
                entry.name
            );
            false
        }
        Ok(None) => {
            println!(
                "{}: SOURCE-ONLY — DDEV identity is not registered",
                entry.name
            );
            true
        }
        Err(error) => {
            println!("{}: NOT READY — {error}", entry.name);
            false
        }
    }
}

fn create(arguments: &ArgMatches) -> ToolResult<u8> {
    let name = arguments
        .get_one::<String>("name")
        .map(String::as_str)
        .ok_or_else(|| ToolError::usage("create requires a workspace name"))?;
    config::validate_name("workspace name", name)?;
    let dry_run = arguments.get_flag("dry-run");
    let source_only = arguments.get_flag("source-only");
    let requested_base = arguments.get_one::<String>("base").map(String::as_str);
    let mut runner = RealCommandRunner::new(dry_run);
    let repository = GitRepository::discover(Path::new("."), &mut runner)?;
    let manager_root = repository.main_worktree.clone();
    let project_config = ProjectConfig::load(&manager_root)?;
    project_config.validate(&manager_root, &mut runner)?;
    let base = repository.resolve_base(requested_base, &mut runner)?;
    let workspace_root = config::safe_join(&manager_root, &project_config.workspace_root)?;
    let workspace = workspace_root.join(name);
    ensure_workspace_path(&workspace_root, &workspace)?;
    let ddev_name = ddev::expected_name(&project_config.project_id, name)?;
    repository.ensure_worktree_available(&workspace, name, &mut runner)?;
    if !source_only {
        validate_local_file_sources(&project_config)?;
    }

    let app_root = (!source_only)
        .then(|| {
            project_config
                .ddev
                .as_ref()
                .map(|ddev_config| workspace.join(&ddev_config.app_root))
        })
        .flatten();
    if let Some(app_root) = &app_root {
        let inspection = ddev::list(&mut runner, &manager_root)?;
        ddev::inspect_new_identity(&inspection, &ddev_name, app_root)?;
    }

    println!("Repository: {}", project_config.project_id);
    println!("Base: {} @ {}", base.reference, base.sha);
    println!("Workspace: {}", workspace.display());
    if dry_run {
        print_create_dry_run(&project_config, &workspace, &base, &ddev_name, source_only);
        return Ok(0);
    }

    let record = OwnershipRecord {
        version: 1,
        project_id: project_config.project_id.clone(),
        common_directory: repository.common_dir.display().to_string(),
        worktree_path: workspace.display().to_string(),
        base_sha: base.sha.clone(),
        branch: name.to_owned(),
        ddev_name,
    };
    let record_path = state::reserve(&repository.common_dir, &record)?;
    match create_after_reservation(
        &repository,
        &project_config,
        &record,
        &workspace,
        &base,
        source_only,
        &mut runner,
    ) {
        Ok(()) => Ok(0),
        Err(error) => Err(preserved_failure(error, &workspace, &record_path)),
    }
}

fn create_after_reservation<R: CommandRunner>(
    repository: &GitRepository,
    project_config: &ProjectConfig,
    record: &OwnershipRecord,
    workspace: &Path,
    base: &git::BaseRevision,
    source_only: bool,
    runner: &mut R,
) -> ToolResult<()> {
    repository.add_worktree(workspace, &record.branch, &base.sha, runner)?;
    let workspace_repository = GitRepository::discover(workspace, runner)?;
    workspace_repository.initialize_submodules(runner)?;
    workspace_repository.checkout_lfs(runner)?;
    let source = workspace_repository.diagnose(Some(&base.sha), runner)?;
    if !source.ready() {
        return Err(ToolError::new(format_source_failure(&source)));
    }
    for warning in &source.warnings {
        println!("Cleanup report: {warning}");
    }
    println!("Source: READY");
    if source_only {
        println!("Runtime: skipped by --source-only");
        println!("DDEV: skipped by --source-only");
        println!("READY — source-only workspace");
        return Ok(());
    }

    prepare_files(project_config, workspace, &workspace_repository, runner)?;
    let mut ddev_project = if let Some(ddev_config) = &project_config.ddev {
        let app_root = config::safe_join(workspace, &ddev_config.app_root)?;
        let inspection = ddev::list(runner, workspace)?;
        ddev::inspect_new_identity(&inspection, &record.ddev_name, &app_root)?;
        ddev::write_override(
            &workspace_repository.root,
            &app_root,
            &record.ddev_name,
            runner,
        )?;
        ddev::start(&app_root, runner)?;
        let after_start = ddev::list(runner, &app_root)?;
        Some(ddev::require_ready_identity(
            &after_start,
            &record.ddev_name,
            &app_root,
        )?)
    } else {
        None
    };
    run_declared_commands(project_config, workspace, runner)?;
    let source = workspace_repository.diagnose(Some(&base.sha), runner)?;
    if !source.ready() {
        return Err(ToolError::new(format_source_failure(&source)));
    }
    workspace_repository.worktree_is_clean(workspace, runner)?;
    for warning in &source.warnings {
        println!("Cleanup report: {warning}");
    }
    if let Some(ddev_config) = &project_config.ddev {
        let app_root = config::safe_join(workspace, &ddev_config.app_root)?;
        let after_commands = ddev::list(runner, &app_root)?;
        ddev_project = Some(ddev::require_ready_identity(
            &after_commands,
            &record.ddev_name,
            &app_root,
        )?);
    }
    if let Some(reason) = runtime_precondition_failure(project_config, workspace) {
        return Err(ToolError::new(format!(
            "runtime readiness failed after declared commands: {reason}"
        )));
    }

    println!("Runtime: READY");
    if let Some(project) = ddev_project {
        println!(
            "DDEV: {} @ {} (running, {})",
            project.name,
            project.approot,
            if project.mutagen_enabled {
                "Mutagen ok"
            } else {
                "Mutagen disabled"
            }
        );
    } else {
        println!("DDEV: skipped (not configured)");
    }
    println!("READY");
    Ok(())
}

fn remove(arguments: &ArgMatches) -> ToolResult<u8> {
    let name = arguments
        .get_one::<String>("name")
        .map(String::as_str)
        .ok_or_else(|| ToolError::usage("remove requires a workspace name"))?;
    config::validate_name("workspace name", name)?;
    let dry_run = arguments.get_flag("dry-run");
    let delete_data = arguments.get_flag("delete-ddev-data");
    let confirmation = arguments.get_one::<String>("confirm").map(String::as_str);
    let data_confirmation = arguments
        .get_one::<String>("confirm-data")
        .map(String::as_str);
    let mut runner = RealCommandRunner::new(false);
    let repository = GitRepository::discover(Path::new("."), &mut runner)?;
    let manager_root = repository.main_worktree.clone();
    let project_config = ProjectConfig::load(&manager_root)?;
    project_config.validate(&manager_root, &mut runner)?;
    let (record_path, record) = state::load(&repository.common_dir, name)?;
    let target = verify_removal_target(&repository, &project_config, name, &record, &mut runner)?;

    println!("Will remove managed worktree {}.", target.path.display());
    if target.ddev.is_some() {
        if delete_data {
            println!(
                "Will stop, remove data, and unlist DDEV project {}.",
                record.ddev_name
            );
        } else {
            println!("Will stop and unlist DDEV project {}.", record.ddev_name);
        }
    }
    println!("Will retain branch {}.", record.branch);
    if delete_data {
        println!("DDEV data deletion requires a second exact confirmation.");
    }
    if dry_run {
        println!("DRY RUN — no DDEV, Git, file, or ownership mutation performed");
        println!("READY — removal preflight complete");
        return Ok(0);
    }

    require_confirmation(name, confirmation, "removal")?;
    if delete_data {
        if target.ddev.is_none() {
            return Err(ToolError::new(
                "--delete-ddev-data was requested but the exact owned DDEV identity is absent",
            ));
        }
        require_confirmation(name, data_confirmation, "DDEV data deletion")?;
    }

    let (_, current_config, current_record) = reload_removal_state(
        &repository,
        name,
        &project_config,
        &record,
        &record_path,
        &mut runner,
    )?;
    let current = verify_removal_target(
        &repository,
        &current_config,
        name,
        &current_record,
        &mut runner,
    )?;
    if current.ddev.is_some() {
        let ddev_config = current_config
            .ddev
            .as_ref()
            .ok_or_else(|| ToolError::new("DDEV identity was found without DDEV configuration"))?;
        let app_root = config::safe_join(&current.path, &ddev_config.app_root)?;
        ddev::stop_unlist(
            &app_root,
            &current_record.ddev_name,
            delete_data,
            &mut runner,
        )?;
    }
    let (latest_record_path, latest_config, latest_record) = reload_removal_state(
        &repository,
        name,
        &project_config,
        &record,
        &record_path,
        &mut runner,
    )?;
    let second_check = verify_removal_target(
        &repository,
        &latest_config,
        name,
        &latest_record,
        &mut runner,
    )?;
    repository.remove_worktree(&second_check.path, &mut runner)?;
    state::delete(&latest_record_path)?;
    println!("READY — managed workspace removed; branch retained");
    Ok(0)
}

#[derive(Debug)]
struct RemovalTarget {
    path: PathBuf,
    ddev: Option<DdevProject>,
}

fn reload_removal_state<R: CommandRunner>(
    repository: &GitRepository,
    name: &str,
    expected_config: &ProjectConfig,
    expected_record: &OwnershipRecord,
    expected_record_path: &Path,
    runner: &mut R,
) -> ToolResult<(PathBuf, ProjectConfig, OwnershipRecord)> {
    let config = ProjectConfig::load(&repository.main_worktree)?;
    config.validate(&repository.main_worktree, runner)?;
    let (record_path, record) = state::load(&repository.common_dir, name)?;
    if record_path != expected_record_path
        || &record != expected_record
        || &config != expected_config
    {
        return Err(ToolError::new(
            "ownership record or project configuration changed after confirmation; no further mutation was attempted",
        ));
    }
    Ok((record_path, config, record))
}

fn verify_removal_target<R: CommandRunner>(
    repository: &GitRepository,
    project_config: &ProjectConfig,
    name: &str,
    record: &OwnershipRecord,
    runner: &mut R,
) -> ToolResult<RemovalTarget> {
    if record.version != 1 {
        return Err(ToolError::new(
            "ownership record version is unsupported; refusing removal",
        ));
    }
    if record.project_id != project_config.project_id {
        return Err(ToolError::new(
            "ownership record project ID differs from current configuration; refusing removal",
        ));
    }
    if Path::new(&record.common_directory) != repository.common_dir {
        return Err(ToolError::new(
            "ownership record points to a different Git common directory; refusing removal",
        ));
    }
    if record.branch != name {
        return Err(ToolError::new(
            "ownership record branch does not match the requested workspace name; refusing removal",
        ));
    }
    if !git::is_full_commit_id(&record.base_sha) {
        return Err(ToolError::new(
            "ownership record base SHA is not a full commit ID; refusing removal",
        ));
    }
    let expected_ddev_name = ddev::expected_name(&project_config.project_id, name)?;
    if record.ddev_name != expected_ddev_name {
        return Err(ToolError::new(
            "ownership record DDEV name does not match the deterministic project identity; refusing removal",
        ));
    }
    let workspace_root =
        config::safe_join(&repository.main_worktree, &project_config.workspace_root)?;
    let expected_path = workspace_root.join(name);
    let path = PathBuf::from(&record.worktree_path);
    if path != expected_path {
        return Err(ToolError::new(format!(
            "ownership record path {} does not match configured workspace path {}; refusing removal",
            path.display(),
            expected_path.display()
        )));
    }
    if path == repository.main_worktree {
        return Err(ToolError::new("the main Git worktree is never removable"));
    }
    if !path.exists() {
        return Err(ToolError::new(format!(
            "managed worktree {} is missing; ownership is not re-created automatically",
            path.display()
        )));
    }
    let canonical = fs::canonicalize(&path)?;
    if canonical != path {
        return Err(ToolError::new(
            "managed worktree path is symlinked or otherwise non-canonical; refusing removal",
        ));
    }
    let worktrees = repository.worktrees(runner)?;
    let Some(worktree) = worktrees.iter().find(|worktree| worktree.path == path) else {
        return Err(ToolError::new(
            "Git does not currently prove the owned worktree path; refusing removal",
        ));
    };
    if worktree.locked {
        return Err(ToolError::new(
            "managed Git worktree is locked; unlock it manually before removal",
        ));
    }
    if worktree.prunable {
        return Err(ToolError::new(
            "managed Git worktree metadata is prunable; repair it manually before removal",
        ));
    }
    if worktree.branch.as_deref() != Some(&format!("refs/heads/{name}")) {
        return Err(ToolError::new(
            "Git worktree branch does not match the ownership record; refusing removal",
        ));
    }
    repository.worktree_is_clean(&path, runner)?;

    let ddev = if let Some(ddev_config) = &project_config.ddev {
        let app_root = config::safe_join(&path, &ddev_config.app_root)?;
        let inspection = ddev::list(runner, &app_root)?;
        find_owned_ddev(&inspection.entries, &record.ddev_name, &app_root)?
    } else {
        None
    };
    Ok(RemovalTarget { path, ddev })
}

fn find_owned_ddev(
    entries: &[DdevProject],
    expected_name: &str,
    app_root: &Path,
) -> ToolResult<Option<DdevProject>> {
    let expected_path = fs::canonicalize(app_root).unwrap_or_else(|_| app_root.to_path_buf());
    let mut exact = Vec::new();
    for entry in entries {
        let entry_path =
            fs::canonicalize(&entry.approot).unwrap_or_else(|_| PathBuf::from(&entry.approot));
        if entry.name == expected_name {
            if entry_path != expected_path {
                return Err(ToolError::new(format!(
                    "DDEV project `{expected_name}` is registered to {}; refusing cross-project cleanup",
                    entry.approot
                )));
            }
            exact.push(entry.clone());
        } else if entry_path == expected_path {
            return Err(ToolError::new(format!(
                "DDEV app root {} is registered under unexpected name `{}`; refusing cleanup",
                app_root.display(),
                entry.name
            )));
        }
    }
    match exact.len() {
        0 => Ok(None),
        1 => Ok(exact.into_iter().next()),
        _ => Err(ToolError::new(format!(
            "DDEV project `{expected_name}` has duplicate registrations; refusing cleanup"
        ))),
    }
}

fn ddev_mutagen_ready(project: &DdevProject) -> bool {
    !project.mutagen_enabled || project.mutagen_status.eq_ignore_ascii_case("ok")
}

fn runtime_precondition_failure(config: &ProjectConfig, workspace: &Path) -> Option<String> {
    for file in &config.files {
        let destination = match config::safe_join(workspace, &file.destination) {
            Ok(destination) => destination,
            Err(error) => return Some(error.to_string()),
        };
        if !is_regular_file(&destination) {
            return Some(format!(
                "file rule `{}` has not materialized regular destination {}",
                file.label,
                destination.display()
            ));
        }
    }
    for check in &config.checks {
        if let Err(error) = run_one_check(check, workspace) {
            return Some(error.to_string());
        }
    }
    None
}

fn prepare_files<R: CommandRunner>(
    project_config: &ProjectConfig,
    workspace: &Path,
    workspace_repository: &GitRepository,
    runner: &mut R,
) -> ToolResult<()> {
    let mut planned = Vec::new();
    for file in &project_config.files {
        let destination = config::safe_join(workspace, &file.destination)?;
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(ToolError::new(format!(
                "file rule `{}` refuses to overwrite existing destination {}",
                file.label,
                destination.display()
            )));
        }
        if !config::is_ignored(&workspace_repository.root, &destination, runner)? {
            return Err(ToolError::new(format!(
                "file rule `{}` destination {} is not ignored by Git; add an ignore rule manually",
                file.label,
                destination.display()
            )));
        }
        let source = match (&file.template, &file.source_env) {
            (Some(template), None) => {
                let path = config::safe_join(workspace, template)?;
                workspace_repository.require_stage_zero_file(&path, runner)?;
                ensure_regular_file(&path, &file.label)?;
                path
            }
            (None, Some(source_env)) => config::source_from_environment(source_env)?,
            _ => {
                return Err(ToolError::new(format!(
                    "file rule `{}` has no unique source",
                    file.label
                )));
            }
        };
        planned.push((file, source, destination));
    }

    for (file, source, destination) in planned {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        copy_file_atomically(
            &source,
            &destination,
            &file.label,
            file.source_env.is_some(),
        )?;
    }
    Ok(())
}

fn copy_file_atomically(
    source: &Path,
    destination: &Path,
    label: &str,
    private: bool,
) -> ToolResult<()> {
    let file_name = destination.file_name().ok_or_else(|| {
        ToolError::new(format!(
            "file rule `{label}` destination {} has no file name",
            destination.display()
        ))
    })?;
    let mut temporary_name = file_name.to_os_string();
    temporary_name.push(".ddev-workspaces.tmp");
    let temporary = destination.with_file_name(temporary_name);
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            ToolError::new(format!(
                "file rule `{label}` temporary destination could not be created: {error}"
            ))
        })?;

    let result: ToolResult<()> = (|| {
        let mut input = fs::File::open(source).map_err(|error| {
            ToolError::new(format!(
                "file rule `{label}` source could not be opened: {error}"
            ))
        })?;
        io::copy(&mut input, &mut output)
            .map_err(|error| ToolError::new(format!("file rule `{label}` copy failed: {error}")))?;
        output.sync_all().map_err(|error| {
            ToolError::new(format!(
                "file rule `{label}` copy could not be synced: {error}"
            ))
        })?;
        if private {
            set_private_permissions(&temporary)?;
        }
        // Publish the complete same-directory file without replacing an existing destination.
        fs::hard_link(&temporary, destination).map_err(|error| {
            ToolError::new(format!(
                "file rule `{label}` destination {} could not be published: {error}",
                destination.display()
            ))
        })?;
        fs::remove_file(&temporary).map_err(|error| {
            ToolError::new(format!(
                "file rule `{label}` temporary destination could not be removed: {error}"
            ))
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn run_declared_commands<R: CommandRunner>(
    project_config: &ProjectConfig,
    workspace: &Path,
    runner: &mut R,
) -> ToolResult<()> {
    for command in &project_config.commands {
        run_one_command(command, workspace, runner)?;
    }
    Ok(())
}

fn run_one_command<R: CommandRunner>(
    command: &CommandRule,
    workspace: &Path,
    runner: &mut R,
) -> ToolResult<()> {
    let cwd = config::safe_join(workspace, &command.cwd)?;
    if !cwd.is_dir() {
        return Err(ToolError::new(format!(
            "declared command `{}` cwd {} is not a directory",
            command.label,
            cwd.display()
        )));
    }
    let mut request =
        crate::command::CommandRequest::new(command.argv[0].clone(), command.argv[1..].to_vec())
            .cwd(&cwd)
            .mutating();
    if command.sensitive {
        request = request.sensitive();
    }
    let output = runner.run(&request)?;
    if output.success() {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "declared command `{}` failed in {} with status {}; remediate it manually and rerun doctor",
            command.label,
            cwd.display(),
            output.status
        )))
    }
}

fn run_one_check(check: &CheckRule, workspace: &Path) -> ToolResult<()> {
    let path = config::safe_join(workspace, &check.path)?;
    match check.kind.as_str() {
        "path-exists" => {
            if fs::symlink_metadata(&path).is_err() {
                return Err(ToolError::new(format!(
                    "readiness check `{}` requires path {}",
                    check.label,
                    path.display()
                )));
            }
        }
        "env-key" => {
            let key = check.key.as_deref().ok_or_else(|| {
                ToolError::new(format!("readiness check `{}` has no key", check.label))
            })?;
            let contents = fs::read_to_string(&path).map_err(|error| {
                ToolError::new(format!(
                    "readiness check `{}` could not read its named file: {error}",
                    check.label
                ))
            })?;
            let present = contents.lines().any(|line| {
                let Some((candidate, value)) = line.split_once('=') else {
                    return false;
                };
                candidate.trim() == key && !unquote(value.trim()).is_empty()
            });
            if !present {
                return Err(ToolError::new(format!(
                    "readiness check `{}` requires a non-empty named environment key",
                    check.label
                )));
            }
        }
        kind => {
            return Err(ToolError::new(format!(
                "readiness check `{}` has unsupported kind `{kind}`",
                check.label
            )));
        }
    }
    Ok(())
}

fn validate_local_file_sources(project_config: &ProjectConfig) -> ToolResult<()> {
    for file in &project_config.files {
        if let Some(source_env) = &file.source_env {
            let _ = config::source_from_environment(source_env)?;
        }
    }
    Ok(())
}

fn ensure_workspace_path(workspace_root: &Path, workspace: &Path) -> ToolResult<()> {
    match fs::symlink_metadata(workspace) {
        Ok(_) => {
            return Err(ToolError::new(format!(
                "workspace path {} already exists; refusing to overwrite it",
                workspace.display()
            )));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(ToolError::new(format!(
                "workspace path {} cannot be inspected: {error}",
                workspace.display()
            )));
        }
        Err(_) => {}
    }
    let root_parent = config::nearest_existing_parent(workspace_root);
    let root = fs::canonicalize(&root_parent)?;
    if root_parent != root {
        return Err(ToolError::new(
            "configured workspace root resolves through a symlink; refusing to create a non-canonical worktree",
        ));
    }
    let parent = config::nearest_existing_parent(workspace);
    let canonical_parent = fs::canonicalize(&parent)?;
    if parent != canonical_parent {
        return Err(ToolError::new(
            "workspace path resolves through a symlink; refusing to create a non-canonical worktree",
        ));
    }
    let parent = canonical_parent;
    if !parent.starts_with(root) {
        return Err(ToolError::new(
            "workspace path escapes the configured ignored workspace root through a symlink",
        ));
    }
    Ok(())
}

fn ensure_regular_file(path: &Path, label: &str) -> ToolResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ToolError::new(format!(
            "file rule `{label}` source {} is unavailable: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(ToolError::new(format!(
            "file rule `{label}` source {} must be a regular non-symlink file",
            path.display()
        )));
    }
    Ok(())
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn set_private_permissions(path: &Path) -> ToolResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn print_source_diagnostics(diagnostics: &SourceDiagnostics) {
    println!(
        "Source: {}",
        if diagnostics.ready() {
            "READY"
        } else {
            "NOT READY"
        }
    );
    for issue in &diagnostics.issues {
        println!("- {issue}");
    }
    for warning in &diagnostics.warnings {
        println!("Cleanup report: {warning}");
    }
}

fn format_source_failure(diagnostics: &SourceDiagnostics) -> String {
    let mut message = String::from("source verification failed");
    for issue in &diagnostics.issues {
        message.push_str("; ");
        message.push_str(issue);
    }
    message
}

fn print_create_dry_run(
    project_config: &ProjectConfig,
    workspace: &Path,
    base: &git::BaseRevision,
    ddev_name: &str,
    source_only: bool,
) {
    println!("Source: planned read-only verification");
    println!(
        "Planned: git worktree add -b {} {} {}",
        workspace
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace"),
        workspace.display(),
        base.sha
    );
    for file in &project_config.files {
        println!("Planned file rule: {} -> {}", file.label, file.destination);
    }
    for command in &project_config.commands {
        if command.sensitive {
            println!("Planned command: {} [sensitive]", command.label);
        } else {
            println!("Planned command: {} in {}", command.label, command.cwd);
        }
    }
    if source_only {
        println!("Runtime: skipped by --source-only");
        println!("DDEV: skipped by --source-only");
    } else if project_config.ddev.is_some() {
        println!("Planned DDEV identity: {ddev_name}");
    } else {
        println!("DDEV: skipped (not configured)");
    }
    println!(
        "READY — dry run complete; no reservation, branch, path, file, or DDEV mutation performed"
    );
}

fn preserved_failure(error: ToolError, workspace: &Path, record_path: &Path) -> ToolError {
    ToolError::new(format!(
        "NOT READY — {error}\nOwnership record preserved at {}; workspace path {} was not rolled back. Run doctor, then remove only after its safety checks pass.",
        record_path.display(),
        workspace.display()
    ))
}

fn require_confirmation(name: &str, provided: Option<&str>, purpose: &str) -> ToolResult<()> {
    if let Some(provided) = provided {
        if provided == name {
            return Ok(());
        }
        return Err(ToolError::new(format!(
            "{purpose} confirmation did not exactly match workspace name `{name}`; no mutation was attempted"
        )));
    }
    if !io::stdin().is_terminal() {
        return Err(ToolError::new(format!(
            "non-interactive {purpose} requires `--confirm {name}`; no mutation was attempted"
        )));
    }
    print!("Type {name} to confirm {purpose}: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if input.trim() == name {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "{purpose} confirmation did not exactly match workspace name `{name}`; no mutation was attempted"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn env_key_check_does_not_return_or_print_the_value() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(".env");
        fs::write(&path, "APP_KEY=secret-value\n").expect("env file");
        let check = CheckRule {
            label: "app key".to_owned(),
            kind: "env-key".to_owned(),
            path: ".env".to_owned(),
            key: Some("APP_KEY".to_owned()),
        };

        run_one_check(&check, directory.path()).expect("key should be ready");
    }

    #[test]
    fn dry_run_output_requests_no_mutating_runner_call() {
        let command = CommandRule {
            label: "setup".to_owned(),
            cwd: ".".to_owned(),
            argv: vec!["printf".to_owned(), "secret".to_owned()],
            sensitive: true,
        };
        let request = crate::command::CommandRequest::new(
            command.argv[0].clone(),
            command.argv[1..].to_vec(),
        )
        .cwd(Path::new("/tmp"))
        .mutating()
        .sensitive();

        assert!(request.sensitive);
        assert!(request.mutating);
    }
}
