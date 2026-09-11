use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::ArgMatches;

use crate::command::{CommandRequest, CommandRunner, RealCommandRunner, ToolError, ToolResult};
use crate::config::{self, DatabaseMode, ProjectConfig, RuntimePaths};
use crate::ddev::{self, HostIdentity};
use crate::git::{self, GitRepository, SourceDiagnostics};
use crate::runtime;
use crate::runtime::PreparedRuntime;
use crate::runtime::database::DatabaseContext;
use crate::runtime::host::HostContext;
use crate::state::{
    self, DatabaseProvenance, DatabaseRecord, OwnedPaths, OwnershipRecord, RECORD_VERSION,
    WorkspaceStatus,
};

pub fn run(matches: ArgMatches) -> ToolResult<u8> {
    let Some((command, arguments)) = matches.subcommand() else {
        return Err(ToolError::usage("one command is required"));
    };
    match command {
        "doctor" => doctor(arguments),
        "host" => host(arguments),
        "list" => list(arguments),
        "create" => create(arguments),
        "remove" => remove(arguments),
        _ => Err(ToolError::usage(format!("unsupported command {command}"))),
    }
}

fn host(arguments: &ArgMatches) -> ToolResult<u8> {
    let Some(("init", _)) = arguments.subcommand() else {
        return Err(ToolError::usage("host requires the init subcommand"));
    };
    let mut runner = RealCommandRunner::new(false);
    let signal_guard = runtime::SignalGuard::install()?;
    let (repository, _config, paths) = load_project(Path::new("."), &mut runner)?;
    require_main_repository(&repository)?;
    let inspected_identity = ddev::inspect_host(&paths.host_root, &mut runner)?;
    let _host_lock = state::acquire_host_lock(&paths.host_root)?;
    let identity = ddev::inspect_host(&paths.host_root, &mut runner)?;
    if !identity.same_runtime_identity(&inspected_identity) {
        return Err(ToolError::new(
            "DDEV host identity changed while waiting for the host lock; retry host initialization",
        ));
    }
    let changed = runtime::host::init(&paths.host_root, &identity, &mut runner)?;
    ensure_operation_not_interrupted(&signal_guard, "shared host initialization")?;
    println!("Host: {}", paths.host_root.display());
    println!("DDEV: {} ({})", identity.project.name, identity.hostname);
    println!(
        "Routing assets: {}",
        if changed {
            "installed"
        } else {
            "already current"
        }
    );
    println!("State: {}", state::state_root(&paths.host_root).display());
    Ok(0)
}

fn create(arguments: &ArgMatches) -> ToolResult<u8> {
    let name = arguments
        .get_one::<String>("name")
        .map(String::as_str)
        .ok_or_else(|| ToolError::usage("create requires a workspace name"))?;
    config::validate_workspace_name(name)?;
    let requested_base = arguments.get_one::<String>("base").map(String::as_str);
    let fresh = arguments.get_flag("fresh");
    let dry_run = arguments.get_flag("dry-run");
    let mut runner = RealCommandRunner::new(dry_run);
    let signal_guard = runtime::SignalGuard::install()?;
    let (repository, project_config, paths) = load_project(Path::new("."), &mut runner)?;
    require_main_repository(&repository)?;
    let host = runtime::host::inspect(&project_config, paths.clone(), &mut runner)?;
    let base = repository.resolve_base(requested_base, &mut runner)?;
    let key = resolve_resource_key(&paths.host_root, &project_config, name)?;
    let requested_database_mode = if fresh {
        DatabaseMode::Fresh
    } else {
        project_config.database_mode()?
    };
    let database = DatabaseContext::from_host(&paths.host_root, &host.identity, &project_config)?;
    let preflight_record = existing_record(&paths.host_root, &project_config, name)?;
    let (workspace, database_name) = if let Some((_, record)) = &preflight_record {
        validate_record_identity(
            record,
            &repository,
            &project_config,
            &paths,
            &host.identity,
            name,
        )?;
        let recorded_mode = database_mode_from_record(record);
        if recorded_mode != requested_database_mode {
            return Err(ToolError::new(format!(
                "workspace {} was reserved with {} database mode; retry with the same configuration and --fresh setting",
                name,
                database_mode_label(recorded_mode)
            )));
        }
        if record.is_retained_data() {
            return Err(ToolError::new(format!(
                "workspace {} retains database {}; remove it with --delete-database before recreating the same name",
                name, record.database.name
            )));
        }
        if matches!(record.status, WorkspaceStatus::Ready) {
            return Err(ToolError::new(format!(
                "workspace {} is already ready at {}",
                name, record.url
            )));
        }
        if matches!(record.status, WorkspaceStatus::Removing) {
            return Err(ToolError::new(format!(
                "workspace {} is being removed; finish removal before retrying create",
                name
            )));
        }
        (
            PathBuf::from(&record.worktree_path),
            record.database.name.clone(),
        )
    } else {
        let workspace = paths.worktree_root.join(&key);
        ensure_unclaimed_runtime_paths(&project_config, &paths, &key)?;
        repository.ensure_worktree_available(&workspace, name, &mut runner)?;
        let database_name = database_name(&key)?;
        if database.exists(&database_name, &mut runner)? {
            return Err(ToolError::new(format!(
                "workspace database {} already exists without a resumable ownership record; unmanaged databases are never adopted",
                database_name
            )));
        }
        (workspace, database_name)
    };
    if dry_run {
        print_create_plan(
            &project_config,
            &host.identity,
            name,
            &key,
            &workspace,
            &base,
            requested_database_mode,
            &database_name,
        );
        return Ok(0);
    }

    state::ensure_state(&paths.host_root)?;
    runtime::ensure_regular_directory(&paths.worktree_root, "configured worktree root")?;
    let identity_lock_key = config::resource_key(&project_config.project_id, name);
    let _workspace_lock = state::acquire_workspace_lock(&paths.host_root, &identity_lock_key)?;
    let locked_key = resolve_resource_key(&paths.host_root, &project_config, name)?;
    if locked_key != key {
        return Err(ToolError::new(
            "workspace resource identity changed while waiting for its lock; retry the operation",
        ));
    }
    let existing = existing_record(&paths.host_root, &project_config, name)?;
    let mut record = if let Some((_, record)) = existing {
        validate_record_identity(
            &record,
            &repository,
            &project_config,
            &paths,
            &host.identity,
            name,
        )?;
        let recorded_mode = database_mode_from_record(&record);
        if recorded_mode != requested_database_mode {
            return Err(ToolError::new(format!(
                "workspace {} was reserved with {} database mode; retry with the same configuration and --fresh setting",
                name,
                database_mode_label(recorded_mode)
            )));
        }
        if record.is_retained_data() {
            return Err(ToolError::new(format!(
                "workspace {} retains database {}; remove it with --delete-database before recreating the same name",
                name, record.database.name
            )));
        }
        if matches!(record.status, WorkspaceStatus::Ready) {
            return Err(ToolError::new(format!(
                "workspace {} is already ready at {}",
                name, record.url
            )));
        }
        if matches!(record.status, WorkspaceStatus::Removing) {
            return Err(ToolError::new(format!(
                "workspace {} is being removed; finish removal before retrying create",
                name
            )));
        }
        record
    } else {
        let _host_lock = state::acquire_host_lock(&paths.host_root)?;
        require_locked_host(&paths.host_root, &host.identity, &mut runner)?;
        repository.ensure_worktree_available(&workspace, name, &mut runner)?;
        if database.exists(&database_name, &mut runner)? {
            return Err(ToolError::new(format!(
                "workspace database {} appeared while reserving the workspace; unmanaged databases are never adopted",
                database_name
            )));
        }
        let record = new_record(
            &repository,
            &project_config,
            &paths,
            &host.identity,
            name,
            &key,
            &base,
            requested_database_mode,
            &database_name,
        )?;
        ensure_operation_not_interrupted(&signal_guard, "workspace reservation")?;
        state::reserve(&paths.host_root, &record)?;
        record
    };
    record.status = WorkspaceStatus::Creating;
    record.error = None;
    state::update(&paths.host_root, &record)?;
    ensure_operation_not_interrupted(&signal_guard, "workspace creation")?;
    match create_reserved(
        &repository,
        &project_config,
        &paths,
        &host,
        &mut record,
        requested_database_mode,
        &signal_guard,
        &mut runner,
    ) {
        Ok(()) => {
            println!("Workspace: {}", record.workspace_name);
            println!("URL: {}", record.url);
            println!("Database: {}", record.database.name);
            println!("READY");
            Ok(0)
        }
        Err(error) => Err(record_failure(&paths.host_root, &mut record, error)),
    }
}

#[allow(clippy::too_many_arguments)]
fn create_reserved<R: CommandRunner>(
    repository: &GitRepository,
    config: &ProjectConfig,
    paths: &RuntimePaths,
    host: &HostContext,
    record: &mut OwnershipRecord,
    database_mode: DatabaseMode,
    signal_guard: &runtime::SignalGuard,
    runner: &mut R,
) -> ToolResult<()> {
    let recorded_mode = database_mode_from_record(record);
    if recorded_mode != database_mode {
        return Err(ToolError::new(
            "requested database mode does not match the ownership record; retry with the original mode",
        ));
    }
    {
        let _host_lock = state::acquire_host_lock(&paths.host_root)?;
        require_locked_host(&paths.host_root, &host.identity, runner)?;
    }
    ensure_operation_not_interrupted(signal_guard, "workspace preparation")?;
    let workspace = Path::new(&record.worktree_path);
    match fs::symlink_metadata(workspace) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ToolError::new(
                "recorded workspace path is a symlink; refusing adoption",
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(ToolError::new(
                "recorded workspace path is not a directory; refusing adoption",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            repository.add_worktree(workspace, &record.branch, &record.base_sha, runner)?;
        }
        Err(error) => return Err(error.into()),
    }
    ensure_operation_not_interrupted(signal_guard, "worktree preparation")?;
    if config.runtime.adapter == "laravel" && record.paths.files.is_empty() {
        let planned = runtime::laravel::planned_files(config, paths, workspace)?;
        if !planned.is_empty()
            && planned
                .iter()
                .all(|path| matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound))
        {
            record.paths.files = planned
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            state::update(&paths.host_root, record)?;
        }
    }
    let workspace_repository =
        validate_owned_worktree(repository, workspace, record, runner, true)?;
    workspace_repository.initialize_submodules(runner)?;
    workspace_repository.checkout_lfs(runner)?;
    let source = workspace_repository.diagnose(Some(&record.base_sha), runner)?;
    if !source.ready() {
        return Err(ToolError::new(format_source_failure(&source)));
    }

    let needs_adapter_prepare = if config.runtime.adapter == "wordpress-component" {
        match record.paths.view.as_deref() {
            None => true,
            Some(view) => {
                let expected_view =
                    state::views_directory(&paths.host_root).join(&record.resource_key);
                if Path::new(view) != expected_view {
                    return Err(ToolError::new(
                        "recorded generated view does not match the resource key",
                    ));
                }
                let files = record
                    .paths
                    .files
                    .iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>();
                let writable = record
                    .paths
                    .writable
                    .iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>();
                match fs::symlink_metadata(view) {
                    Ok(_) => !runtime::wordpress::view_is_complete(
                        config,
                        Path::new(view),
                        workspace,
                        &record.resource_key,
                        &files,
                        &writable,
                    )?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                    Err(error) => return Err(error.into()),
                }
            }
        }
    } else if config.runtime.adapter == "laravel" {
        laravel_runtime_needs_prepare(record)?
    } else {
        return Err(ToolError::new(format!(
            "unsupported runtime adapter {}",
            config.runtime.adapter
        )));
    };
    let prepared = if needs_adapter_prepare {
        if config.runtime.adapter == "wordpress-component" {
            record.paths.files.clear();
            record.paths.writable.clear();
            let view = state::views_directory(&paths.host_root).join(&record.resource_key);
            let view_exists = match fs::symlink_metadata(&view) {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            if view_exists {
                if runtime::wordpress::view_is_owned(&view, &record.resource_key)? {
                    runtime::wordpress::remove_owned_view(&view, &record.resource_key)?;
                } else {
                    return Err(ToolError::new(format!(
                        "WordPress generated view {} already exists without a complete ownership record; inspect it manually",
                        view.display()
                    )));
                }
            } else {
                record.paths.view = Some(view.display().to_string());
                state::update(&paths.host_root, record)?;
            }
        }
        let prepared = prepare_adapter(
            config,
            paths,
            workspace,
            &record.resource_key,
            &record.database.name,
            &record.url,
        )?;
        record.paths = owned_paths(&prepared, &record.paths.link);
        state::update(&paths.host_root, record)?;
        prepared
    } else {
        prepared_from_record(config, paths, workspace, record, false)?
    };
    ensure_operation_not_interrupted(signal_guard, "runtime preparation")?;

    let runtime_root = prepared.view.as_deref().unwrap_or(workspace);
    let database = DatabaseContext::from_host(&paths.host_root, &host.identity, config)?;
    if !record.database.prepared {
        if record.database.ownership_token.is_none() {
            record.database.ownership_token =
                Some(database_ownership_token(config, &record.workspace_name)?);
            state::update(&paths.host_root, record)?;
        }
        let ownership_token = record
            .database
            .ownership_token
            .as_deref()
            .ok_or_else(|| ToolError::new("workspace database ownership token is missing"))?;
        if database.exists(&record.database.name, runner)? {
            if !database.owns(&record.database.name, ownership_token, runner)? {
                return Err(ToolError::new(format!(
                    "workspace database {} already exists without a completed ownership record; inspect and remove it manually before retrying",
                    record.database.name
                )));
            }
            record.database.prepared = true;
            state::update(&paths.host_root, record)?;
        } else {
            database.prepare(
                &record.database.name,
                database_mode,
                ownership_token,
                runner,
            )?;
            record.database.prepared = true;
            state::update(&paths.host_root, record)?;
        }
    } else if !database.exists(&record.database.name, runner)? {
        return Err(ToolError::new(format!(
            "workspace database {} was recorded as prepared but is missing; inspect the retained ownership record before retrying",
            record.database.name
        )));
    } else if record.database.ownership_token.is_none() {
        return Err(ToolError::new(
            "workspace database ownership token is missing from a prepared record; inspect it manually before retrying",
        ));
    }
    ensure_operation_not_interrupted(signal_guard, "database preparation")?;
    run_provision(config, runtime_root, database_mode, runner)?;
    run_checks(config, runtime_root)?;
    ensure_only_owned_changes(&workspace_repository, &prepared, runner)?;
    ensure_route_target_is_safe(config, &prepared.route_target, workspace, &paths.host_root)?;
    ensure_operation_not_interrupted(signal_guard, "workspace checks")?;

    {
        let _host_lock = state::acquire_host_lock(&paths.host_root)?;
        require_locked_host(&paths.host_root, &host.identity, runner)?;
        ddev::publish_link(
            &paths.host_root,
            &record.resource_key,
            &prepared.route_target,
        )?;
    }
    if signal_guard.interrupted() {
        let rollback = rollback_route(
            &paths.host_root,
            &host.identity,
            &record.resource_key,
            &prepared.route_target,
            runner,
        );
        return match rollback {
            Ok(()) => Err(ToolError::new(
                "workspace creation interrupted after route publication; route was rolled back",
            )),
            Err(rollback_error) => Err(ToolError::new(format!(
                "workspace creation interrupted after route publication; route rollback also failed: {rollback_error}"
            ))),
        };
    }
    if host.identity.project.mutagen_enabled
        && let Err(error) = ddev::sync_mutagen(&paths.host_root, &record.resource_key, runner)
    {
        let rollback = rollback_route(
            &paths.host_root,
            &host.identity,
            &record.resource_key,
            &prepared.route_target,
            runner,
        );
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(ToolError::new(format!(
                "{error}; route rollback also failed: {rollback_error}"
            ))),
        };
    }
    record.status = WorkspaceStatus::Ready;
    record.error = None;
    if let Err(error) = state::update(&paths.host_root, record) {
        let rollback = rollback_route(
            &paths.host_root,
            &host.identity,
            &record.resource_key,
            &prepared.route_target,
            runner,
        );
        if let Err(rollback_error) = rollback {
            return Err(ToolError::new(format!(
                "could not persist the ready workspace record: {error}; route rollback also failed: {rollback_error}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn rollback_route<R: CommandRunner>(
    host_root: &Path,
    identity: &HostIdentity,
    resource_key: &str,
    route_target: &Path,
    runner: &mut R,
) -> ToolResult<()> {
    let _host_lock = state::acquire_host_lock(host_root)?;
    require_locked_host(host_root, identity, runner)?;
    ddev::unpublish_link(host_root, resource_key, route_target)
}

fn remove(arguments: &ArgMatches) -> ToolResult<u8> {
    let name = arguments
        .get_one::<String>("name")
        .map(String::as_str)
        .ok_or_else(|| ToolError::usage("remove requires a workspace name"))?;
    config::validate_workspace_name(name)?;
    let dry_run = arguments.get_flag("dry-run");
    let delete_database = arguments.get_flag("delete-database");
    let yes = arguments.get_flag("yes");
    let mut runner = RealCommandRunner::new(false);
    let signal_guard = runtime::SignalGuard::install()?;
    let (repository, config, paths) = load_project(Path::new("."), &mut runner)?;
    require_main_repository(&repository)?;
    let identity_lock_key = config::resource_key(&config.project_id, name);
    let _workspace_lock = state::acquire_workspace_lock(&paths.host_root, &identity_lock_key)?;
    let (record_path, mut record) =
        existing_record(&paths.host_root, &config, name)?.ok_or_else(|| {
            ToolError::new(format!(
                "no ownership record for workspace {name}; unmanaged resources are never removed"
            ))
        })?;
    let host = runtime::host::inspect(&config, paths.clone(), &mut runner)?;
    validate_record_identity(&record, &repository, &config, &paths, &host.identity, name)?;
    ensure_operation_not_interrupted(&signal_guard, "workspace removal")?;

    if record.is_retained_data() {
        validate_retained_record(&record, &paths)?;
        if !delete_database {
            return Err(ToolError::new(format!(
                "workspace {} retains database {}; pass --delete-database to remove the retained data",
                name, record.database.name
            )));
        }
        println!(
            "Will delete retained database {} and ownership record {}; branch {} remains.",
            record.database.name,
            record_path.display(),
            record.branch
        );
        if dry_run {
            println!("DRY RUN — no database or ownership mutation performed");
            return Ok(0);
        }
        require_confirmation(name, yes)?;
        ensure_operation_not_interrupted(&signal_guard, "workspace removal confirmation")?;
        let (current_record_path, current_record) =
            state::load(&paths.host_root, &record.resource_key)?;
        if current_record_path != record_path || current_record != record {
            return Err(ToolError::new(
                "retained workspace ownership changed after confirmation; rerun removal",
            ));
        }
        let _host_lock = state::acquire_host_lock(&paths.host_root)?;
        require_locked_host(&paths.host_root, &host.identity, &mut runner)?;
        validate_retained_record(&record, &paths)?;
        let database = DatabaseContext::from_host(&paths.host_root, &host.identity, &config)?;
        if database.exists(&record.database.name, &mut runner)? {
            let ownership_token = record.database.ownership_token.as_deref().ok_or_else(|| {
                ToolError::new("retained workspace database ownership token is missing")
            })?;
            database.drop_owned(&record.database.name, ownership_token, &mut runner)?;
        }
        state::delete(&record_path)?;
        println!(
            "Removed retained data for {name}; branch {} remains.",
            record.branch
        );
        return Ok(0);
    }

    let workspace = Path::new(&record.worktree_path);
    let has_owned_runtime = workspace.exists()
        || record.paths.view.is_some()
        || !record.paths.files.is_empty()
        || !record.paths.writable.is_empty();
    let prepared = if has_owned_runtime {
        Some(prepared_from_record(
            &config,
            &paths,
            workspace,
            &record,
            !matches!(record.status, WorkspaceStatus::Ready),
        )?)
    } else {
        match fs::symlink_metadata(&record.paths.link) {
            Ok(_) => {
                return Err(ToolError::new(
                    "workspace record has no materialized runtime, but its route still exists; refusing cleanup without exact ownership proof",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        }
    };
    let workspace_repository = match fs::symlink_metadata(workspace) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ToolError::new(
                "recorded workspace path is a symlink; refusing removal",
            ));
        }
        Ok(metadata) if metadata.is_dir() => Some(validate_owned_worktree(
            &repository,
            workspace,
            &record,
            &mut runner,
            false,
        )?),
        Ok(_) => {
            return Err(ToolError::new(
                "recorded workspace path is not a directory; refusing removal",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if let (Some(workspace_repository), Some(prepared)) = (&workspace_repository, prepared.as_ref())
    {
        ensure_only_owned_changes(workspace_repository, prepared, &mut runner)?;
    }
    let database = DatabaseContext::from_host(&paths.host_root, &host.identity, &config)?;
    let database_exists = database.exists(&record.database.name, &mut runner)?;
    let database_owned = if !record.database.prepared && database_exists {
        record
            .database
            .ownership_token
            .as_deref()
            .map(|token| database.owns(&record.database.name, token, &mut runner))
            .transpose()?
            .unwrap_or(false)
    } else {
        record.database.prepared
    };
    if database_exists && !database_owned {
        return Err(ToolError::new(format!(
            "workspace database {} exists but database ownership was not durably completed; refusing automatic deletion even with --delete-database; inspect and remove it manually before retrying",
            record.database.name
        )));
    }
    println!("Will remove workspace path {}.", workspace.display());
    println!("Will remove route {}.", record.url);
    println!("Will retain branch {}.", record.branch);
    if database_owned
        && matches!(record.database.provenance, DatabaseProvenance::Copy)
        && !delete_database
    {
        println!(
            "Will retain copied database {} as retained-data ownership.",
            record.database.name
        );
    } else {
        println!("Will delete database {}.", record.database.name);
    }
    if dry_run {
        println!("DRY RUN — no route, file, Git, database, or ownership mutation performed");
        return Ok(0);
    }
    require_confirmation(name, yes)?;
    ensure_operation_not_interrupted(&signal_guard, "workspace removal confirmation")?;

    let (_, current_record) = state::load(&paths.host_root, &record.resource_key)?;
    validate_record_identity(
        &current_record,
        &repository,
        &config,
        &paths,
        &host.identity,
        name,
    )?;
    if current_record != record {
        return Err(ToolError::new(
            "workspace ownership changed after confirmation; rerun removal",
        ));
    }
    {
        let _host_lock = state::acquire_host_lock(&paths.host_root)?;
        require_locked_host(&paths.host_root, &host.identity, &mut runner)?;
        let locked_database_exists = database.exists(&record.database.name, &mut runner)?;
        let locked_database_owned = if !record.database.prepared && locked_database_exists {
            record
                .database
                .ownership_token
                .as_deref()
                .map(|token| database.owns(&record.database.name, token, &mut runner))
                .transpose()?
                .unwrap_or(false)
        } else {
            record.database.prepared
        };
        if locked_database_exists && !locked_database_owned {
            return Err(ToolError::new(format!(
                "workspace database {} appeared without durable ownership during removal; refusing automatic deletion; inspect and remove it manually before retrying",
                record.database.name
            )));
        }
        if locked_database_owned && !record.database.prepared {
            record.database.prepared = true;
        }
        record.status = WorkspaceStatus::Removing;
        record.error = Some("removal in progress; rerun remove to continue if it fails".to_owned());
        state::update(&paths.host_root, &record)?;
        if let Some(prepared) = prepared.as_ref() {
            ddev::unpublish_link(
                &paths.host_root,
                &record.resource_key,
                &prepared.route_target,
            )?;
        }
    }
    ensure_operation_not_interrupted(&signal_guard, "route removal")?;
    if let Some(prepared) = prepared.as_ref() {
        cleanup_adapter(&config, prepared, workspace, &mut runner)?;
    }
    ensure_operation_not_interrupted(&signal_guard, "runtime cleanup")?;
    if let Some(workspace_repository) = workspace_repository {
        workspace_repository.worktree_is_clean(workspace, &mut runner)?;
        repository.remove_worktree(workspace, &mut runner)?;
    }
    ensure_operation_not_interrupted(&signal_guard, "worktree removal")?;
    let retain_database = record.database.prepared
        && matches!(record.database.provenance, DatabaseProvenance::Copy)
        && !delete_database;
    {
        let _host_lock = state::acquire_host_lock(&paths.host_root)?;
        require_locked_host(&paths.host_root, &host.identity, &mut runner)?;
        if !retain_database
            && record.database.prepared
            && database.exists(&record.database.name, &mut runner)?
        {
            let ownership_token =
                record.database.ownership_token.as_deref().ok_or_else(|| {
                    ToolError::new("workspace database ownership token is missing")
                })?;
            database.drop_owned(&record.database.name, ownership_token, &mut runner)?;
        }
        if retain_database {
            record.status = WorkspaceStatus::RetainedData;
            record.error = None;
            record.paths.view = None;
            record.paths.files.clear();
            record.paths.writable.clear();
            state::update(&paths.host_root, &record)?;
        } else {
            state::delete(&record_path)?;
        }
    }
    if retain_database {
        println!(
            "Removed workspace {}; retained database {}. Branch {} remains.",
            name, record.database.name, record.branch
        );
    } else {
        println!(
            "Removed workspace {name}; branch {} remains.",
            record.branch
        );
    }
    Ok(0)
}

fn list(arguments: &ArgMatches) -> ToolResult<u8> {
    let _ = arguments;
    let mut runner = RealCommandRunner::new(false);
    let (repository, config, paths) = load_project(Path::new("."), &mut runner)?;
    let entries = current_project_entries(state::list(&paths.host_root)?, &repository, &config);
    println!("Repository: {}", repository.main_worktree.display());
    println!("Host: {}", paths.host_root.display());
    if entries.is_empty() {
        println!("No managed workspaces.");
        return Ok(0);
    }
    let host_status = ddev::inspect_host(&paths.host_root, &mut runner)
        .map(|identity| format!("{} ({})", identity.project.name, identity.hostname))
        .unwrap_or_else(|error| format!("unavailable: {error}"));
    println!("DDEV host: {host_status}");
    let mut ready = true;
    for entry in entries {
        match entry.record {
            Ok(record) => {
                println!(
                    "{}  {}  {}  {}",
                    record.workspace_name,
                    record.status.label(),
                    record.url,
                    record.database.name
                );
                if let Some(error) = record.error {
                    println!("  error: {error}");
                }
                if !matches!(record.status, WorkspaceStatus::Ready) {
                    ready = false;
                }
            }
            Err(error) => {
                ready = false;
                println!("{}  invalid-record  {error}", entry.name);
            }
        }
    }
    if ready {
        Ok(0)
    } else {
        println!("NOT READY — one or more records require attention");
        Ok(1)
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
    let config = ProjectConfig::load(&repository.main_worktree)?;
    let paths = config.validate_structure(&repository.main_worktree, &mut runner)?;
    println!("Configuration: READY");
    let mut ready = diagnostics.ready();
    let host_identity = match ddev::inspect_host(&paths.host_root, &mut runner) {
        Ok(identity) => {
            println!(
                "DDEV host: READY — {} ({})",
                identity.project.name, identity.hostname
            );
            match ddev::require_initialized(&paths.host_root, &identity) {
                Ok(()) => println!("Routing: READY"),
                Err(error) => {
                    println!("Routing: NOT READY — {error}");
                    ready = false;
                }
            }
            Some(identity)
        }
        Err(error) => {
            println!("DDEV host: NOT READY — {error}");
            ready = false;
            None
        }
    };
    let entries = current_project_entries(state::list(&paths.host_root)?, &repository, &config);
    for entry in entries {
        match entry.record {
            Ok(record) => {
                println!(
                    "Workspace {}: {} — {}",
                    record.workspace_name,
                    record.status.label(),
                    record.url
                );
                if !matches!(record.status, WorkspaceStatus::Ready) {
                    ready = false;
                }
                if let Some(error) = &record.error {
                    println!("  error: {error}");
                    ready = false;
                }
                if matches!(record.status, WorkspaceStatus::Ready) {
                    if let Some(identity) = host_identity.as_ref() {
                        if let Err(error) = inspect_ready_record(
                            &record,
                            &repository,
                            &config,
                            &paths,
                            identity,
                            &mut runner,
                        ) {
                            println!("  NOT READY — {error}");
                            ready = false;
                        }
                    } else {
                        println!("  NOT READY — live host checks are unavailable");
                        ready = false;
                    }
                } else if matches!(record.status, WorkspaceStatus::RetainedData)
                    && let Some(identity) = host_identity.as_ref()
                    && let Err(error) =
                        inspect_retained_record(&record, &paths, identity, &mut runner)
                {
                    println!("  NOT READY — {error}");
                    ready = false;
                }
            }
            Err(error) => {
                println!("Workspace {}: INVALID RECORD — {error}", entry.name);
                ready = false;
            }
        }
    }
    if ready {
        println!("READY");
        Ok(0)
    } else {
        println!("NOT READY");
        Ok(1)
    }
}

fn current_project_entries(
    entries: Vec<state::RecordEntry>,
    repository: &GitRepository,
    config: &ProjectConfig,
) -> Vec<state::RecordEntry> {
    entries
        .into_iter()
        .filter(|entry| match &entry.record {
            Ok(record) => {
                record.project_id == config.project_id
                    && record.repository_root == repository.main_worktree.display().to_string()
            }
            Err(_) => true,
        })
        .collect()
}

fn inspect_ready_record<R: CommandRunner>(
    record: &OwnershipRecord,
    repository: &GitRepository,
    config: &ProjectConfig,
    paths: &RuntimePaths,
    identity: &HostIdentity,
    runner: &mut R,
) -> ToolResult<()> {
    validate_record_identity(
        record,
        repository,
        config,
        paths,
        identity,
        &record.workspace_name,
    )?;
    let workspace = Path::new(&record.worktree_path);
    if !workspace.is_dir() {
        return Err(ToolError::new("recorded worktree is missing"));
    }
    let prepared = prepared_from_record(config, paths, workspace, record, false)?;
    ensure_route_target_is_safe(config, &prepared.route_target, workspace, &paths.host_root)?;
    let link = Path::new(&record.paths.link);
    let target = ddev::link_target(link)
        .map_err(|error| ToolError::new(format!("recorded route link is unavailable: {error}")))?;
    let resolved_target = if target.is_absolute() {
        target
    } else {
        link.parent()
            .ok_or_else(|| ToolError::new("recorded route link has no parent"))?
            .join(target)
    };
    if fs::canonicalize(resolved_target)? != fs::canonicalize(&prepared.route_target)? {
        return Err(ToolError::new(
            "recorded route link points to a different runtime target",
        ));
    }
    let database = DatabaseContext::from_host(&paths.host_root, identity, config)?;
    if !database.exists(&record.database.name, runner)? {
        return Err(ToolError::new(format!(
            "recorded workspace database {} is missing",
            record.database.name
        )));
    }
    let ownership_token =
        record.database.ownership_token.as_deref().ok_or_else(|| {
            ToolError::new("recorded workspace database ownership token is missing")
        })?;
    if !database.owns(&record.database.name, ownership_token, runner)? {
        return Err(ToolError::new(format!(
            "recorded workspace database {} is missing its ownership marker",
            record.database.name
        )));
    }
    Ok(())
}

fn inspect_retained_record<R: CommandRunner>(
    record: &OwnershipRecord,
    paths: &RuntimePaths,
    identity: &HostIdentity,
    runner: &mut R,
) -> ToolResult<()> {
    validate_retained_record(record, paths)?;
    let database = DatabaseContext::from_host_source(
        &paths.host_root,
        identity,
        record.database.source.as_deref(),
    )?;
    if !database.exists(&record.database.name, runner)? {
        return Err(ToolError::new(format!(
            "retained workspace database {} is missing",
            record.database.name
        )));
    }
    let ownership_token =
        record.database.ownership_token.as_deref().ok_or_else(|| {
            ToolError::new("retained workspace database ownership token is missing")
        })?;
    if !database.owns(&record.database.name, ownership_token, runner)? {
        return Err(ToolError::new(format!(
            "retained workspace database {} is missing its ownership marker",
            record.database.name
        )));
    }
    Ok(())
}

fn load_project<R: CommandRunner>(
    path: &Path,
    runner: &mut R,
) -> ToolResult<(GitRepository, ProjectConfig, RuntimePaths)> {
    let repository = GitRepository::discover(path, runner)?;
    let config = ProjectConfig::load(&repository.main_worktree)?;
    let paths = config.validate(&repository.main_worktree, runner)?;
    Ok((repository, config, paths))
}

fn require_main_repository(repository: &GitRepository) -> ToolResult<()> {
    if repository.root != repository.main_worktree {
        return Err(ToolError::new(
            "workspace lifecycle commands must run from the main checkout; use doctor PATH for a managed worktree",
        ));
    }
    Ok(())
}

fn resolve_resource_key(
    host_root: &Path,
    config: &ProjectConfig,
    workspace_name: &str,
) -> ToolResult<String> {
    let base = config::resource_key(&config.project_id, workspace_name);
    let entries = state::list(host_root)?;
    if let Some(entry) = entries.iter().find(|entry| entry.name == base) {
        match &entry.record {
            Ok(record)
                if record.project_id == config.project_id
                    && record.workspace_name == workspace_name =>
            {
                return Ok(base);
            }
            Ok(_) => {}
            Err(error) => {
                return Err(ToolError::new(format!(
                    "resource key {} has an invalid ownership record: {error}; repair it manually",
                    base
                )));
            }
        }
    } else {
        let suffix = config::identity_hash(&config.project_id, workspace_name);
        let candidate = format!("{base}-{suffix}");
        if let Some(entry) = entries.iter().find(|entry| entry.name == candidate) {
            match &entry.record {
                Ok(record)
                    if record.project_id == config.project_id
                        && record.workspace_name == workspace_name =>
                {
                    return Ok(candidate);
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(ToolError::new(format!(
                        "resource key {} has an invalid ownership record: {error}; repair it manually",
                        candidate
                    )));
                }
            }
            return Err(ToolError::new(format!(
                "resource key collision remains after identity hashing: {candidate}; repair the host records manually"
            )));
        }
        return Ok(base);
    }
    let suffix = config::identity_hash(&config.project_id, workspace_name);
    let candidate = format!("{base}-{suffix}");
    if let Some(entry) = entries.iter().find(|entry| entry.name == candidate) {
        match &entry.record {
            Ok(record)
                if record.project_id == config.project_id
                    && record.workspace_name == workspace_name =>
            {
                return Ok(candidate);
            }
            Ok(_) => {}
            Err(error) => {
                return Err(ToolError::new(format!(
                    "resource key collision remains after identity hashing: {candidate} has an invalid ownership record: {error}; repair it manually"
                )));
            }
        }
        return Err(ToolError::new(format!(
            "resource key collision remains after identity hashing: {candidate}; repair the host records manually"
        )));
    }
    Ok(candidate)
}

fn existing_record(
    host_root: &Path,
    config: &ProjectConfig,
    workspace_name: &str,
) -> ToolResult<Option<(PathBuf, OwnershipRecord)>> {
    let entries = state::list(host_root)?;
    let matches = entries
        .into_iter()
        .filter_map(|entry| match entry.record {
            Ok(record)
                if record.project_id == config.project_id
                    && record.workspace_name == workspace_name =>
            {
                Some(Ok((state::record_path(host_root, &entry.name), record)))
            }
            Err(error)
                if entry.name == config::resource_key(&config.project_id, workspace_name) =>
            {
                Some(Err(ToolError::new(format!(
                    "matching ownership record {} is invalid: {error}",
                    entry.name
                ))))
            }
            _ => None,
        })
        .collect::<Result<Vec<_>, ToolError>>()?;
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(ToolError::new(
            "multiple ownership records match the workspace identity; resolve them manually",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn new_record(
    repository: &GitRepository,
    config: &ProjectConfig,
    paths: &RuntimePaths,
    identity: &HostIdentity,
    workspace_name: &str,
    resource_key: &str,
    base: &git::BaseRevision,
    database_mode: DatabaseMode,
    database_name: &str,
) -> ToolResult<OwnershipRecord> {
    let url = ddev::expected_url(&identity.hostname, resource_key)?;
    let ownership_token = database_ownership_token(config, workspace_name)?;
    Ok(OwnershipRecord {
        version: RECORD_VERSION,
        project_id: config.project_id.clone(),
        workspace_name: workspace_name.to_owned(),
        resource_key: resource_key.to_owned(),
        adapter: config.runtime.adapter.clone(),
        repository_root: repository.main_worktree.display().to_string(),
        common_directory: repository.common_dir.display().to_string(),
        host_root: paths.host_root.display().to_string(),
        worktree_path: paths.worktree_root.join(resource_key).display().to_string(),
        base_sha: base.sha.clone(),
        branch: workspace_name.to_owned(),
        hostname: ddev::expected_hostname(&identity.hostname, resource_key)?,
        url,
        database: DatabaseRecord {
            name: database_name.to_owned(),
            provenance: match database_mode {
                DatabaseMode::Copy => DatabaseProvenance::Copy,
                DatabaseMode::Fresh => DatabaseProvenance::Fresh,
            },
            prepared: false,
            ownership_token: Some(ownership_token),
            source: match database_mode {
                DatabaseMode::Copy => Some(
                    config
                        .database
                        .source
                        .clone()
                        .unwrap_or_else(|| "db".to_owned()),
                ),
                DatabaseMode::Fresh => None,
            },
        },
        paths: OwnedPaths {
            link: state::links_directory(&paths.host_root)
                .join(resource_key)
                .display()
                .to_string(),
            view: None,
            files: Vec::new(),
            writable: Vec::new(),
        },
        status: WorkspaceStatus::Creating,
        error: None,
    })
}

fn database_name(resource_key: &str) -> ToolResult<String> {
    let name = if resource_key.len() <= 57 {
        format!("dw_{}", resource_key.replace('-', "_"))
    } else {
        let prefix = &resource_key[..48];
        let suffix = &resource_key[resource_key.len().saturating_sub(8)..];
        format!("dw_{}_{}", prefix.replace('-', "_"), suffix)
    };
    config::validate_database_name("workspace database", &name)?;
    Ok(name)
}

fn ensure_unclaimed_runtime_paths(
    config: &ProjectConfig,
    paths: &RuntimePaths,
    resource_key: &str,
) -> ToolResult<()> {
    let link = state::links_directory(&paths.host_root).join(resource_key);
    match fs::symlink_metadata(&link) {
        Ok(_) => {
            return Err(ToolError::new(format!(
                "workspace route {} already exists without a resumable ownership record; refusing to adopt it",
                link.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if config.runtime.adapter == "wordpress-component" {
        let view = state::views_directory(&paths.host_root).join(resource_key);
        match fs::symlink_metadata(&view) {
            Ok(_) => {
                return Err(ToolError::new(format!(
                    "WordPress generated view {} already exists without a resumable ownership record; refusing to adopt it",
                    view.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn database_ownership_token(config: &ProjectConfig, workspace_name: &str) -> ToolResult<String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ToolError::new(format!("system clock error: {error}")))?
        .as_nanos();
    Ok(format!(
        "{}-{nonce:x}",
        config::identity_hash(&config.project_id, workspace_name)
    ))
}

fn database_mode_from_record(record: &OwnershipRecord) -> DatabaseMode {
    match record.database.provenance {
        DatabaseProvenance::Copy => DatabaseMode::Copy,
        DatabaseProvenance::Fresh => DatabaseMode::Fresh,
    }
}

fn validate_record_identity(
    record: &OwnershipRecord,
    repository: &GitRepository,
    config: &ProjectConfig,
    paths: &RuntimePaths,
    identity: &HostIdentity,
    workspace_name: &str,
) -> ToolResult<()> {
    if record.version != RECORD_VERSION
        || record.project_id != config.project_id
        || record.workspace_name != workspace_name
        || record.repository_root != repository.main_worktree.display().to_string()
        || record.common_directory != repository.common_dir.display().to_string()
        || record.host_root != paths.host_root.display().to_string()
        || record.adapter != config.runtime.adapter
        || record.branch != workspace_name
    {
        return Err(ToolError::new(
            "ownership record identity does not match the current repository, host, adapter, or workspace name",
        ));
    }
    let expected_key = config::resource_key(&config.project_id, workspace_name);
    let hashed_key = format!(
        "{}-{}",
        expected_key,
        config::identity_hash(&config.project_id, workspace_name)
    );
    if record.resource_key != expected_key && record.resource_key != hashed_key {
        return Err(ToolError::new(
            "ownership record resource key does not match the deterministic workspace identity",
        ));
    }
    let expected_database = database_name(&record.resource_key)?;
    if record.database.name != expected_database {
        return Err(ToolError::new(
            "ownership record database name does not match the deterministic resource key",
        ));
    }
    let expected_link = state::links_directory(&paths.host_root).join(&record.resource_key);
    if Path::new(&record.paths.link) != expected_link {
        return Err(ToolError::new(
            "ownership record route link does not match the host-local resource identity",
        ));
    }
    match record.database.provenance {
        DatabaseProvenance::Copy => {
            let expected_source = config.database.source.as_deref().unwrap_or("db");
            if record.database.source.as_deref().unwrap_or("db") != expected_source {
                return Err(ToolError::new(
                    "ownership record database source does not match the current configuration",
                ));
            }
        }
        DatabaseProvenance::Fresh if record.database.source.is_some() => {
            return Err(ToolError::new(
                "fresh ownership record unexpectedly contains a source database",
            ));
        }
        DatabaseProvenance::Fresh => {}
    }
    let expected_hostname = ddev::expected_hostname(&identity.hostname, &record.resource_key)?;
    if record.hostname != expected_hostname {
        return Err(ToolError::new(
            "ownership record hostname does not match the current shared host identity",
        ));
    }
    let expected_worktree = paths.worktree_root.join(&record.resource_key);
    if Path::new(&record.worktree_path) != expected_worktree {
        return Err(ToolError::new(
            "ownership record worktree path does not match the configured worktree root",
        ));
    }
    Ok(())
}

fn validate_retained_record(record: &OwnershipRecord, paths: &RuntimePaths) -> ToolResult<()> {
    if !record.database.prepared
        || !matches!(record.database.provenance, DatabaseProvenance::Copy)
        || record.database.source.is_none()
        || record.database.ownership_token.is_none()
        || record.paths.view.is_some()
        || !record.paths.files.is_empty()
        || !record.paths.writable.is_empty()
    {
        return Err(ToolError::new(
            "retained-data ownership record is incomplete or has runtime paths; inspect it manually before deleting the database",
        ));
    }
    let expected_link = state::links_directory(&paths.host_root).join(&record.resource_key);
    if Path::new(&record.paths.link) != expected_link {
        return Err(ToolError::new(
            "retained-data ownership record route link is outside the host-local links directory",
        ));
    }
    match fs::symlink_metadata(&record.paths.link) {
        Ok(_) => {
            return Err(ToolError::new(
                "retained-data ownership record still has a route link; inspect and remove the route manually before deleting the database",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    match fs::symlink_metadata(&record.worktree_path) {
        Ok(_) => Err(ToolError::new(
            "retained-data ownership record still has a worktree; inspect it manually before deleting the database",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_route_target_is_safe(
    config: &ProjectConfig,
    route_target: &Path,
    workspace: &Path,
    host_root: &Path,
) -> ToolResult<()> {
    runtime::ensure_tree_within(route_target, host_root)?;
    if config.runtime.adapter == "laravel"
        && matches!(
            fs::symlink_metadata(workspace),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink()
        )
    {
        runtime::ensure_tree_within(route_target, workspace)?;
    } else if config.runtime.adapter == "wordpress-component" {
        runtime::wordpress::ensure_selected_component_is_safe(config, route_target, workspace)?;
    }
    Ok(())
}

fn prepare_adapter(
    config: &ProjectConfig,
    paths: &RuntimePaths,
    workspace: &Path,
    resource_key: &str,
    database: &str,
    url: &str,
) -> ToolResult<PreparedRuntime> {
    match config.runtime.adapter.as_str() {
        "laravel" => runtime::laravel::prepare(config, paths, workspace, database, url),
        "wordpress-component" => {
            runtime::wordpress::prepare(config, paths, workspace, resource_key, database, url)
        }
        adapter => Err(ToolError::new(format!(
            "unsupported runtime adapter {adapter}"
        ))),
    }
}

fn laravel_runtime_needs_prepare(record: &OwnershipRecord) -> ToolResult<bool> {
    if record.paths.view.is_some() {
        return Err(ToolError::new(
            "Laravel ownership record unexpectedly contains a generated view",
        ));
    }
    let mut materialized = false;
    let mut missing = false;
    for path in &record.paths.files {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(ToolError::new(format!(
                    "recorded Laravel generated file {} is not a regular file",
                    path
                )));
            }
            Ok(_) => materialized = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing = true,
            Err(error) => return Err(error.into()),
        }
    }
    for path in &record.paths.writable {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ToolError::new(format!(
                    "recorded Laravel writable path {} is not a regular directory",
                    path
                )));
            }
            Ok(_) => materialized = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing = true,
            Err(error) => return Err(error.into()),
        }
    }
    if missing && materialized {
        return Err(ToolError::new(
            "Laravel runtime preparation is incomplete; inspect the recorded generated paths before retrying",
        ));
    }
    Ok(missing
        || (!materialized && record.paths.files.is_empty() && record.paths.writable.is_empty()))
}

fn prepared_from_record(
    config: &ProjectConfig,
    paths: &RuntimePaths,
    workspace: &Path,
    record: &OwnershipRecord,
    allow_missing_route_target: bool,
) -> ToolResult<PreparedRuntime> {
    let expected_link = state::links_directory(&paths.host_root).join(&record.resource_key);
    if Path::new(&record.paths.link) != expected_link {
        return Err(ToolError::new(
            "recorded route link is outside the host-local links directory",
        ));
    }
    let route_target = if let Some(view) = &record.paths.view {
        let expected_view = state::views_directory(&paths.host_root).join(&record.resource_key);
        if Path::new(view) != expected_view {
            return Err(ToolError::new(
                "recorded generated view does not match the resource key",
            ));
        }
        PathBuf::from(view)
    } else {
        let docroot = config.runtime.docroot.as_deref().unwrap_or(".");
        if allow_missing_route_target && !workspace.exists() {
            config::validate_repository_relative("runtime.docroot", docroot)?;
            workspace.join(config::normalize_relative_path(docroot))
        } else {
            config::safe_join(workspace, docroot)?
        }
    };
    validate_owned_path(&route_target, &paths.host_root, allow_missing_route_target)?;
    if config.runtime.adapter == "wordpress-component"
        && record.paths.view.is_some()
        && matches!(fs::symlink_metadata(&route_target), Ok(metadata) if metadata.is_dir())
    {
        if !runtime::wordpress::view_is_owned(&route_target, &record.resource_key)? {
            return Err(ToolError::new(format!(
                "recorded WordPress generated view {} is not proven workspace-owned; inspect it manually",
                route_target.display()
            )));
        }
        if matches!(
            fs::symlink_metadata(workspace),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink()
        ) && !runtime::wordpress::selected_component_is_owned(config, &route_target, workspace)?
        {
            return Err(ToolError::new(format!(
                "recorded WordPress generated view {} does not point at the recorded worktree component; inspect it manually",
                route_target.display()
            )));
        }
    }
    match fs::symlink_metadata(&route_target) {
        Ok(route_metadata) => {
            if route_metadata.file_type().is_symlink() || !route_metadata.is_dir() {
                return Err(ToolError::new(format!(
                    "recorded route target {} is missing; inspect the ownership record manually",
                    route_target.display()
                )));
            }
        }
        Err(error)
            if allow_missing_route_target && error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ToolError::new(format!(
                "recorded route target {} cannot be inspected: {error}",
                route_target.display()
            )));
        }
    }
    if fs::symlink_metadata(&route_target).is_ok() {
        ensure_route_target_is_safe(config, &route_target, workspace, &paths.host_root)?;
    }
    let files = record
        .paths
        .files
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let writable = record
        .paths
        .writable
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if config.runtime.adapter == "laravel" {
        runtime::laravel::validate_recorded_paths(config, paths, workspace, &files, &writable)?;
    } else if config.runtime.adapter == "wordpress-component" {
        runtime::wordpress::validate_recorded_paths(&route_target, &files, &writable)?;
    }
    let allowed_writable = if config.runtime.adapter == "laravel" {
        runtime::laravel::configured_writable_paths(config, workspace)?
    } else {
        writable.clone()
    };
    let owner_root = record
        .paths
        .view
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.to_path_buf());
    for path in files.iter().chain(writable.iter()) {
        validate_owned_path(path, &owner_root, allow_missing_route_target)?;
    }
    Ok(PreparedRuntime {
        route_target,
        view: record.paths.view.as_deref().map(PathBuf::from),
        files,
        writable,
        allowed_writable,
    })
}

fn validate_owned_path(path: &Path, owner_root: &Path, allow_missing: bool) -> ToolResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        || !path.starts_with(owner_root)
    {
        return Err(ToolError::new(
            "recorded generated paths escape their workspace or generated view",
        ));
    }
    let root_metadata = match fs::symlink_metadata(owner_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !allow_missing {
                return Err(ToolError::new(format!(
                    "recorded generated path {} is missing; inspect the ownership record manually",
                    path.display()
                )));
            }
            return match fs::symlink_metadata(path) {
                Ok(_) => Err(ToolError::new(
                    "recorded generated path exists while its owner root is missing; refusing removal",
                )),
                Err(path_error) if path_error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(path_error) => Err(path_error.into()),
            };
        }
        Err(error) => {
            return Err(ToolError::new(format!(
                "owned path root {} cannot be inspected: {error}",
                owner_root.display()
            )));
        }
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(ToolError::new(format!(
            "owned path root {} is not a regular directory",
            owner_root.display()
        )));
    }
    let canonical_root = fs::canonicalize(owner_root)?;
    let relative = path.strip_prefix(owner_root).map_err(|_| {
        ToolError::new("recorded generated paths escape their workspace or generated view")
    })?;
    let mut current = owner_root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(ToolError::new(
                "recorded generated paths contain an unsafe path component",
            ));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ToolError::new(format!(
                    "owned generated path {} has a symlinked ancestor; refusing removal",
                    current.display()
                )));
            }
            Ok(_) => {
                if !fs::canonicalize(&current)?.starts_with(&canonical_root) {
                    return Err(ToolError::new(
                        "recorded generated paths escape their workspace or generated view",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !allow_missing {
                    return Err(ToolError::new(format!(
                        "recorded generated path {} is missing; inspect the ownership record manually",
                        path.display()
                    )));
                }
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn validate_owned_worktree<R: CommandRunner>(
    repository: &GitRepository,
    workspace: &Path,
    record: &OwnershipRecord,
    runner: &mut R,
    require_base_head: bool,
) -> ToolResult<GitRepository> {
    let canonical_workspace = fs::canonicalize(workspace).map_err(|error| {
        ToolError::new(format!(
            "cannot resolve recorded workspace {}: {error}",
            workspace.display()
        ))
    })?;
    let workspace_repository = GitRepository::discover(workspace, runner)?;
    if workspace_repository.root != canonical_workspace {
        return Err(ToolError::new(
            "recorded workspace path does not resolve to its own Git worktree; refusing adoption",
        ));
    }
    if workspace_repository.common_dir != repository.common_dir {
        return Err(ToolError::new(
            "workspace path belongs to another Git common directory; refusing adoption",
        ));
    }
    let expected_branch = format!("refs/heads/{}", record.branch);
    let registered = repository.worktrees(runner)?.into_iter().find(|worktree| {
        fs::canonicalize(&worktree.path)
            .map(|path| path == canonical_workspace)
            .unwrap_or(false)
    });
    if registered
        .as_ref()
        .and_then(|worktree| worktree.branch.as_deref())
        != Some(expected_branch.as_str())
    {
        return Err(ToolError::new(
            "recorded workspace path is not registered on the recorded branch; refusing adoption",
        ));
    }
    if require_base_head && workspace_repository.current_head(runner)? != record.base_sha {
        return Err(ToolError::new(
            "workspace HEAD differs from the reserved base commit; inspect the retained record manually",
        ));
    }
    Ok(workspace_repository)
}

fn owned_paths(prepared: &PreparedRuntime, link: &str) -> OwnedPaths {
    OwnedPaths {
        link: link.to_owned(),
        view: prepared
            .view
            .as_ref()
            .map(|path| path.display().to_string()),
        files: prepared
            .files
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        writable: prepared
            .writable
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    }
}

fn cleanup_adapter<R: CommandRunner>(
    config: &ProjectConfig,
    prepared: &PreparedRuntime,
    workspace: &Path,
    runner: &mut R,
) -> ToolResult<()> {
    match config.runtime.adapter.as_str() {
        "laravel" => runtime::laravel::cleanup(prepared, workspace, runner),
        "wordpress-component" => runtime::wordpress::cleanup(prepared),
        adapter => Err(ToolError::new(format!(
            "unsupported runtime adapter {adapter}"
        ))),
    }
}

fn require_locked_host<R: CommandRunner>(
    host_root: &Path,
    expected: &HostIdentity,
    runner: &mut R,
) -> ToolResult<()> {
    let current = ddev::inspect_host(host_root, runner)?;
    if !current.same_runtime_identity(expected) {
        return Err(ToolError::new(
            "DDEV host identity changed while waiting for the host lock; retry the operation",
        ));
    }
    ddev::require_initialized(host_root, &current)
}

fn ensure_operation_not_interrupted(
    signal_guard: &runtime::SignalGuard,
    phase: &str,
) -> ToolResult<()> {
    if signal_guard.interrupted() {
        return Err(ToolError::new(format!(
            "operation interrupted during {phase}; rerun the command to reconcile the ownership record"
        )));
    }
    Ok(())
}

fn run_provision<R: CommandRunner>(
    config: &ProjectConfig,
    runtime_root: &Path,
    mode: DatabaseMode,
    runner: &mut R,
) -> ToolResult<()> {
    for provision in &config.provision {
        if provision.when != "always"
            && provision.when
                != match mode {
                    DatabaseMode::Copy => "copy",
                    DatabaseMode::Fresh => "fresh",
                }
        {
            continue;
        }
        let cwd = config::safe_join(runtime_root, &provision.cwd)?;
        let program = provision
            .argv
            .first()
            .ok_or_else(|| ToolError::new("provision rule has an empty argv"))?
            .clone();
        let request = CommandRequest::new(program, provision.argv.iter().skip(1).cloned())
            .cwd(&cwd)
            .mutating();
        let request = if provision.sensitive {
            request.sensitive()
        } else {
            request
        };
        let output = runner.run(&request)?;
        if !output.success() {
            return Err(ToolError::new(format!(
                "provision rule {} failed; inspect the retained ownership record before retrying",
                provision.label.as_deref().unwrap_or("unnamed")
            )));
        }
    }
    Ok(())
}

fn run_checks(config: &ProjectConfig, runtime_root: &Path) -> ToolResult<()> {
    for check in &config.checks {
        let path = config::safe_join(runtime_root, &check.path)?;
        match check.kind.as_str() {
            "path-exists" if path.exists() => {}
            "path-exists" => {
                return Err(ToolError::new(format!(
                    "readiness check {} requires path {}",
                    check.label.as_deref().unwrap_or(&check.path),
                    path.display()
                )));
            }
            "env-key" => {
                let key = check
                    .key
                    .as_deref()
                    .ok_or_else(|| ToolError::new("environment readiness check has no key"))?;
                let content = fs::read_to_string(&path).map_err(|error| {
                    ToolError::new(format!(
                        "readiness check {} could not read its named file: {error}",
                        check.label.as_deref().unwrap_or(&check.path)
                    ))
                })?;
                if !content.lines().any(|line| {
                    line.split_once('=')
                        .is_some_and(|(name, value)| name.trim() == key && !value.trim().is_empty())
                }) {
                    return Err(ToolError::new(format!(
                        "readiness check {} requires non-empty environment key {}",
                        check.label.as_deref().unwrap_or(&check.path),
                        key
                    )));
                }
            }
            _ => {
                return Err(ToolError::new(format!(
                    "unsupported readiness check {}",
                    check.label.as_deref().unwrap_or(&check.path)
                )));
            }
        }
    }
    Ok(())
}

fn ensure_only_owned_changes<R: CommandRunner>(
    repository: &GitRepository,
    prepared: &PreparedRuntime,
    runner: &mut R,
) -> ToolResult<()> {
    let request = CommandRequest::new(
        "git",
        [
            "status",
            "--porcelain=v1",
            "-z",
            "--ignored=matching",
            "--untracked-files=all",
        ],
    )
    .cwd(&repository.root);
    let output = runner.run(&request)?;
    if !output.success() {
        return Err(ToolError::new(
            "could not inspect workspace changes after provisioning",
        ));
    }
    let allowed = prepared
        .files
        .iter()
        .chain(prepared.allowed_writable.iter())
        .filter_map(|path| path.strip_prefix(&repository.root).ok())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    let mut unexpected = Vec::new();
    for entry in output.stdout.split('\0').filter(|entry| !entry.is_empty()) {
        let path = entry.get(3..).unwrap_or(entry).replace('\\', "/");
        if !allowed.iter().any(|candidate| {
            path == *candidate
                || path.starts_with(&format!("{candidate}/"))
                || candidate.starts_with(&format!("{path}/"))
        }) {
            unexpected.push(path);
        }
    }
    if unexpected.is_empty() {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "workspace contains changes outside declared generated or writable paths: {}; inspect them manually",
            unexpected.join(", ")
        )))
    }
}

fn record_failure(host_root: &Path, record: &mut OwnershipRecord, error: ToolError) -> ToolError {
    let message = error.to_string();
    record.status = WorkspaceStatus::Error;
    record.error = Some(message.clone());
    match state::update(host_root, record) {
        Ok(_) => ToolError::new(format!(
            "{message}\nOwnership record retained at {}; run list, doctor, retry create, or remove after inspection",
            state::record_path(host_root, &record.resource_key).display()
        )),
        Err(update_error) => ToolError::new(format!(
            "{message}\nAdditionally, ownership record could not be updated: {update_error}"
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn print_create_plan(
    config: &ProjectConfig,
    identity: &HostIdentity,
    name: &str,
    key: &str,
    workspace: &Path,
    base: &git::BaseRevision,
    mode: DatabaseMode,
    database: &str,
) {
    println!("Repository: {}", config.project_id);
    println!("Host: {} ({})", identity.project.name, identity.hostname);
    println!("Workspace: {name}");
    println!("Resource key: {key}");
    println!("Base: {} @ {}", base.reference, base.sha);
    println!("Worktree: {}", workspace.display());
    println!(
        "URL: {}",
        ddev::expected_url(&identity.hostname, key).unwrap_or_else(|_| "invalid".to_owned())
    );
    println!("Database: {database} ({})", database_mode_label(mode));
    println!("Adapter: {}", config.runtime.adapter);
    println!("DRY RUN — no route, file, Git, database, or ownership mutation performed");
}

fn database_mode_label(mode: DatabaseMode) -> &'static str {
    match mode {
        DatabaseMode::Copy => "copy",
        DatabaseMode::Fresh => "fresh",
    }
}

fn require_confirmation(name: &str, yes: bool) -> ToolResult<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(ToolError::new(format!(
            "removing workspace {name} requires --yes in a non-interactive terminal"
        )));
    }
    print!("Remove workspace {name}? Type 'yes' to continue: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if answer.trim() == "yes" {
        Ok(())
    } else {
        Err(ToolError::new("removal cancelled"))
    }
}

fn print_source_diagnostics(diagnostics: &SourceDiagnostics) {
    for issue in &diagnostics.issues {
        println!("Git: NOT READY — {issue}");
    }
    for warning in &diagnostics.warnings {
        println!("Git: WARNING — {warning}");
    }
    if diagnostics.ready() {
        println!("Git: READY");
    }
}

fn format_source_failure(diagnostics: &SourceDiagnostics) -> String {
    if diagnostics.issues.is_empty() {
        "Git source diagnostics did not produce a ready result".to_owned()
    } else {
        format!("Git source is not ready: {}", diagnostics.issues.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_names_are_safe_and_bounded() {
        let name = database_name(&"a".repeat(52)).unwrap();
        assert!(name.len() <= 60);
        assert!(name.starts_with("dw_"));
    }

    #[test]
    fn database_mode_labels_are_stable() {
        assert_eq!(database_mode_label(DatabaseMode::Copy), "copy");
        assert_eq!(database_mode_label(DatabaseMode::Fresh), "fresh");
    }
}
