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

    // Stamp the `--graph` pin before dispatch so every command resolves the
    // same graph root. `set_explicit_graph` is a OnceLock, so dispatch's later
    // call is a harmless no-op.
    if let Some(g) = &cli.graph {
        db::set_explicit_graph(g);
    }

    commands::dispatch(cli)
}
