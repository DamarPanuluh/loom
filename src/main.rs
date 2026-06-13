use anyhow::Result;

mod agent;
mod cli;
mod commands;
mod db;
mod gate;
mod output;
mod repo;
mod saga;
#[cfg(feature = "treesitter")]
mod ts_imports;
mod types;

fn main() -> Result<()> {
    // parse_or_teach: any syntax failure appends the failing command's
    // EXAMPLE block — errors teach instead of stalling the loop.
    let cli = cli::parse_or_teach();
    commands::dispatch(cli)
}
