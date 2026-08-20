//! `eruser serve` — start the local web interface.

use super::{Error, Paths};
use crate::history::Store;
use crate::template::Engine;
use crate::web::Server;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Port to listen on
    #[arg(long, short, default_value_t = 8080)]
    pub port: u16,

    /// Address to bind. Leave this at localhost unless you understand the
    /// consequences: the interface has no authentication yet, so anything
    /// that can reach it can read your profile and send mail as you.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Do not open a browser window
    #[arg(long)]
    pub no_browser: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            port: 8080,
            host: "127.0.0.1".to_string(),
            no_browser: false,
        }
    }
}

pub use crate::web::Error as ServeError;

pub async fn run(paths: &Paths, args: Args) -> Result<(), Error> {
    let config_path = paths.config_path();

    // A missing or broken config is not fatal here: the wizard exists to fix
    // exactly that, so the server starts and sends the visitor to it.
    let config = if config_path.exists() {
        match crate::config::Config::load_lenient(&config_path) {
            Ok((config, warning)) => {
                if let Some(warning) = warning {
                    eprintln!("warning: {warning}");
                }
                Some(config)
            }
            Err(error) => {
                eprintln!("warning: the config could not be read: {error}");
                eprintln!("The setup wizard will let you enter it again.");
                None
            }
        }
    } else {
        None
    };

    let brokers = paths.load_brokers()?;
    let store = Store::open(Store::default_path()).await?;
    let engine = Engine::new()?;

    let server = Server::new(
        &args.host,
        args.port,
        config,
        config_path,
        brokers,
        store.clone(),
        engine,
    )?;

    let url = format!("http://{}:{}", display_host(&args.host), args.port);
    println!("eruser is running at {url}");
    if args.host != "127.0.0.1" && args.host != "localhost" {
        println!();
        println!("Warning: this is reachable from outside this machine, and the");
        println!("interface has no password. Anyone who can reach it can read your");
        println!("details and send mail from your account.");
    }
    println!("Press Ctrl-C to stop.");

    if !args.no_browser {
        open_browser(&url);
    }

    let result = server.serve(shutdown_signal()).await;
    store.close().await;
    result?;

    println!("Stopped.");
    Ok(())
}

/// `0.0.0.0` is a bind address, not somewhere to point a browser.
fn display_host(host: &str) -> &str {
    match host {
        "0.0.0.0" | "::" => "localhost",
        other => other,
    }
}

/// Resolves on Ctrl-C, or on SIGTERM where that exists.
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            // Without the signal handler, Ctrl-C alone still works.
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }

    println!("\nShutting down…");
}

/// Open the default browser, ignoring failure.
///
/// A headless machine or a missing xdg-open is not a reason to refuse to
/// serve; the URL is printed either way.
fn open_browser(url: &str) {
    let (command, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/c", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };

    let _ = std::process::Command::new(command)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
