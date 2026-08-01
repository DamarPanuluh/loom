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
        let rendered = format!("{e:#}");
        eprintln!("error: {rendered}");
        // A parent loom that spawned this one distinguishes "my own lock got in
        // the way" from a real failure by this exit code, not by scraping the
        // message — so a failing test that prints the marker cannot be
        // misread as an infrastructure block.
        if rendered.contains(loom::store::LOCK_CONTENTION_MARKER) {
            std::process::exit(loom::store::LOCK_CONTENTION_EXIT_CODE);
        }
        std::process::exit(1);
    }
}
