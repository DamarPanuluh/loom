use anyhow::Result;
use clap::Parser;

mod agent;
mod cli;
mod commands;
mod db;
mod gate;
mod output;
mod repo;
mod saga;
mod types;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    commands::dispatch(cli)
}
