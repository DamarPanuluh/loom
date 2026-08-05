use clap::Parser;
use loom::cli::Cli;

fn main() {
    // Consume any parent-issued contention capability before parsing arguments
    // or running code that could spawn a subprocess. The FD becomes
    // close-on-exec and its environment name disappears process-wide.
    loom::subprocess::initialize_contention_capability();

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
        // A parent loom recognizes infrastructure contention only when this
        // reserved exit code is paired with an out-of-band attestation on the
        // private descriptor it supplied for this observation. The human marker
        // remains useful diagnostics but is never trusted for classification.
        if rendered.contains(loom::store::LOCK_CONTENTION_MARKER)
            || rendered.contains(loom::harness::HARNESS_CONTENTION_MARKER)
        {
            loom::subprocess::attest_contention_from_env();
            std::process::exit(loom::LOCK_CONTENTION_EXIT_CODE);
        }
        std::process::exit(1);
    }
}
