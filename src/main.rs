mod cli;
mod command;
mod config;
mod ddev;
mod git;
mod state;
mod workspace;

use std::process::ExitCode;

fn main() -> ExitCode {
    let matches = cli::build_cli().get_matches();
    match workspace::run(matches) {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            let message = error.to_string();
            eprintln!("{message}");
            if !message.lines().any(|line| line.starts_with("NOT READY")) {
                eprintln!("NOT READY");
            }
            ExitCode::from(error.exit_code())
        }
    }
}
