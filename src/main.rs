use anyhow::Result;

mod agent;
mod cli;
mod commands;
mod db;
mod gate;
mod output;
mod repo;
mod saga;
mod serve;
#[cfg(feature = "treesitter")]
mod ts_imports;
mod types;

fn main() -> Result<()> {
    // parse_or_teach: any syntax failure appends the failing command's
    // EXAMPLE block — errors teach instead of stalling the loop.
    let cli = cli::parse_or_teach();

    // Stamp the `--graph` pin NOW (before the client resolves its root), not
    // only inside `dispatch` — otherwise `try_client`'s `resolve_root()` would
    // miss the flag and target the wrong graph (cwd). `set_explicit_graph` is a
    // OnceLock set, so `dispatch`'s later call is a harmless no-op.
    if let Some(g) = &cli.graph {
        db::set_explicit_graph(g);
    }

    // The OPT-IN daemon fast path. `loom serve` itself must NEVER attempt to be
    // its own client (it IS the daemon); every other command tries the daemon
    // first, but ONLY when `LOOM_DAEMON=1` AND `--json` — `try_client` returns
    // None (⇒ fall straight through to direct dispatch) in every other case and
    // on ANY error, so behaviour is byte-identical to today when the daemon is
    // off or anything goes wrong.
    if !matches!(cli.command, Some(cli::Command::Serve { .. })) {
        if let Some(result) =
            serve::try_client(cli.json, &std::env::args().skip(1).collect::<Vec<_>>())
        {
            return result;
        }
    }

    commands::dispatch(cli)
}
