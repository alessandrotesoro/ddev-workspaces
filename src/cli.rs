use clap::{Arg, ArgAction, Command};

pub fn build_cli() -> Command {
    Command::new("ddev-workspaces")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Create and manage conservative local Git/DDEV workspaces")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .subcommand(
            Command::new("doctor")
                .about("Diagnose a repository or managed workspace without changing it")
                .arg(Arg::new("path").value_name("PATH").index(1)),
        )
        .subcommand(
            Command::new("list").about("List managed workspaces for the current repository"),
        )
        .subcommand(
            Command::new("create")
                .about("Create an isolated workspace")
                .arg(
                    Arg::new("base").long("base").value_name("REV").help(
                        "Use one locally resolvable commit instead of origin's advertised HEAD",
                    ),
                )
                .arg(
                    Arg::new("source-only")
                        .long("source-only")
                        .action(ArgAction::SetTrue)
                        .help("Stop after source readiness; skip runtime preparation and DDEV"),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help("Run safe preflight checks and print planned mutations"),
                )
                .arg(Arg::new("name").value_name("NAME").required(true).index(1)),
        )
        .subcommand(
            Command::new("remove")
                .about("Remove one proven managed workspace while retaining its branch")
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help("Run safety checks and print exact owned targets without mutation"),
                )
                .arg(
                    Arg::new("delete-ddev-data")
                        .long("delete-ddev-data")
                        .action(ArgAction::SetTrue)
                        .help("Also request DDEV's snapshot-by-default data removal"),
                )
                .arg(
                    Arg::new("yes")
                        .long("yes")
                        .action(ArgAction::SetTrue)
                        .help("Skip the interactive removal confirmation"),
                )
                .arg(Arg::new("name").value_name("NAME").required(true).index(1)),
        )
}
