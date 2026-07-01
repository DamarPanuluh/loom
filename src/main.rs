use clap::Parser;
use loom::cli::Cli;

fn main() {
    let cli = Cli::parse();
    if let Err(e) = loom::commands::run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
