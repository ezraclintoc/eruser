use clap::Parser;

use eruser::cli::Cli;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // RUST_LOG controls verbosity; without it, only warnings and errors from
    // the library reach the terminal, so normal output stays readable.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .without_time()
        .init();

    if let Err(error) = Cli::parse().run().await {
        // Print the whole chain: the top-level message names what failed and
        // the sources say why.
        eprintln!("error: {}", eruser::send::error_chain(&error));
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
