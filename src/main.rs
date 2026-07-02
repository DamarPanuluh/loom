use clap::Parser;
use loom::cli::Cli;

fn main() {
    // `loom … | head` must terminate quietly when the downstream reader closes
    // the pipe — a panic trace would rattle any driver (human or LLM). Rust
    // ignores SIGPIPE by default; restore the platform default.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    if let Err(e) = loom::commands::run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
