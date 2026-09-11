use clap::{Arg, ArgAction, Command};

pub fn build_cli() -> Command {
    Command::new("ddev-workspaces")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Create and manage shared-host Git/DDEV workspaces")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .subcommand(
            Command::new("doctor")
                .about("Diagnose a repository, host, or managed workspace without changing it")
                .arg(Arg::new("path").value_name("PATH").index(1)),
        )
        .subcommand(
            Command::new("host")
                .about("Manage the shared DDEV host integration")
                .subcommand_required(true)
                .subcommand(
                    Command::new("init")
                        .about("Install the owned wildcard route on the configured running host"),
                ),
        )
        .subcommand(Command::new("list").about("List managed workspaces for the shared host"))
        .subcommand(
            Command::new("create")
                .about("Create an isolated workspace on the initialized shared host")
                .arg(
                    Arg::new("base").long("base").value_name("REV").help(
                        "Use one locally resolvable commit instead of origin's advertised HEAD",
                    ),
                )
                .arg(
                    Arg::new("fresh")
                        .long("fresh")
                        .action(ArgAction::SetTrue)
                        .help("Create an empty workspace database and run fresh provisioning"),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help("Run read-only checks and print planned mutations"),
                )
                .arg(Arg::new("name").value_name("NAME").required(true).index(1)),
        )
        .subcommand(
            Command::new("remove")
                .about("Remove one proven workspace while retaining its source branch")
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help("Run safety checks and print exact owned targets without mutation"),
                )
                .arg(
                    Arg::new("delete-database")
                        .long("delete-database")
                        .action(ArgAction::SetTrue)
                        .help("Explicitly delete an owned workspace database"),
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
