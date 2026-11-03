//! `mqcat` — a kcat-style command-line producer and consumer for IBM MQ.
//!
//! The command line follows the shape of [kcat](https://github.com/edenhill/kcat):
//! a mode flag selects producing or consuming, a target names the queue or
//! topic, and an optional format string controls how consumed messages are
//! rendered. See the crate README for the mapping between kcat concepts and
//! IBM MQ.

mod cli;
mod connection;
mod consume;
mod delimiters;
mod error;
mod format;
mod locale;
mod produce;
mod target;

use std::process::ExitCode;

use clap::Parser as _;

use crate::cli::Cli;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error::report(&err);
            err.exit_code()
        }
    }
}

fn run(cli: Cli) -> Result<(), error::MqCatError> {
    cli.validate()?;
    let target = cli.target()?;

    match cli.mode() {
        cli::Mode::Produce => produce::run(&cli, &target),
        cli::Mode::Consume => consume::run(&cli, &target),
        cli::Mode::List => consume::list(&cli, &target),
    }
}
