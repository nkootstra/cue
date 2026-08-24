//! Diagnostic logging setup.
//!
//! User-facing output stays on stdout; tracing writes to stderr so the CLI
//! can be composed in pipes. `--verbose` raises the default level, and
//! `RUST_LOG` always wins for fine-grained control.

pub fn init(verbose: bool) {
    let default_filter = if verbose { "debug" } else { "warn" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}
