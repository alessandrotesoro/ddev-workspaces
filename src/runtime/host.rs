use std::path::Path;

use crate::command::{CommandRunner, ToolResult};
use crate::config::{ProjectConfig, RuntimePaths};
use crate::ddev::{self, HostIdentity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostContext {
    pub paths: RuntimePaths,
    pub identity: HostIdentity,
}

pub fn inspect<R: CommandRunner>(
    _config: &ProjectConfig,
    paths: RuntimePaths,
    runner: &mut R,
) -> ToolResult<HostContext> {
    let identity = ddev::inspect_host(&paths.host_root, runner)?;
    ddev::require_initialized(&paths.host_root, &identity)?;
    Ok(HostContext { paths, identity })
}

pub fn init<R: CommandRunner>(
    host_root: &Path,
    identity: &HostIdentity,
    runner: &mut R,
) -> ToolResult<bool> {
    ddev::init_host(host_root, identity, runner)
}
