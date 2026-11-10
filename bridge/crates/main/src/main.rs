// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Bridge binary: reads the configuration and wires the crates together.
//!
//! ```text
//! nebula-bridge serve --config bridge.toml
//! nebula-bridge rewrap-keys --config bridge.toml
//! nebula-bridge rotate-vapid-key --config bridge.toml
//! nebula-bridge discover <domain> [--private-network] [--no-ispdb]
//! nebula-bridge hash-secret        < secret   (empty input: generate one)
//! nebula-bridge gen-master-key
//! ```

#![forbid(unsafe_code)]

mod backend;
mod config;
mod discover;
mod vapid;

use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context as _, bail};
use data_encoding::BASE64;
use nebula_crypto::{ClientSecretHash, Keyring, Token};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use crate::config::{CertificateCheck, Config, MASTER_KEY_ENV};

const USAGE: &str = "\
Usage:
  nebula-bridge serve --config <file>   Run the bridge
  nebula-bridge rewrap-keys --config <file>
                                         Wrap the accounts' data keys with the current
                                         master key (also done when the bridge starts)
  nebula-bridge rotate-vapid-key --config <file>
                                         Delete the Web Push (VAPID) key and every push
                                         subscription; a new key is made at the next start
  nebula-bridge discover <domain> [--private-network] [--no-ispdb]
                                         Find a domain's mail servers and print them as
                                         configuration to review. No password is sent, and
                                         the bridge changes nothing by itself
  nebula-bridge hash-secret             Read an OAuth client secret from stdin and print
                                         its argon2id hash (empty input: generate a secret)
  nebula-bridge gen-master-key          Print a new random master key (base64)
  nebula-bridge --version
";

/// How often expired tokens and ended sessions are cleaned up.
const CLEANUP_EVERY: Duration = Duration::from_secs(60);

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve { config: PathBuf },
    RewrapKeys { config: PathBuf },
    RotateVapidKey { config: PathBuf },
    Discover(discover::Arguments),
    HashSecret,
    GenMasterKey,
    Version,
    Help,
}

fn parse_args(args: &[String]) -> anyhow::Result<Command> {
    match args {
        [cmd, flag, path] if cmd == "serve" && flag == "--config" => Ok(Command::Serve {
            config: PathBuf::from(path),
        }),
        [cmd, flag, path] if cmd == "rewrap-keys" && flag == "--config" => {
            Ok(Command::RewrapKeys {
                config: PathBuf::from(path),
            })
        }
        [cmd, flag, path] if cmd == "rotate-vapid-key" && flag == "--config" => {
            Ok(Command::RotateVapidKey {
                config: PathBuf::from(path),
            })
        }
        [cmd, rest @ ..] if cmd == "discover" => {
            Ok(Command::Discover(discover::Arguments::parse(rest)?))
        }
        [cmd] if cmd == "hash-secret" => Ok(Command::HashSecret),
        [cmd] if cmd == "gen-master-key" => Ok(Command::GenMasterKey),
        [cmd] if cmd == "--version" || cmd == "-V" => Ok(Command::Version),
        [] => Ok(Command::Help),
        [cmd] if cmd == "--help" || cmd == "-h" || cmd == "help" => Ok(Command::Help),
        _ => bail!("unknown arguments\n\n{USAGE}"),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse_args(&args).and_then(run);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Errors carry no secrets (crate errors never include them).
            let _ = writeln!(io::stderr().lock(), "error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> anyhow::Result<()> {
    let mut out = io::stdout().lock();
    match command {
        Command::Help => out.write_all(USAGE.as_bytes())?,
        Command::Version => writeln!(out, "nebula-bridge {}", env!("CARGO_PKG_VERSION"))?,
        Command::GenMasterKey => writeln!(out, "{}", generate_master_key()?)?,
        Command::HashSecret => hash_secret(&mut out)?,
        Command::Serve { config } => {
            drop(out);
            serve(&config)?;
        }
        Command::RewrapKeys { config } => {
            drop(out);
            rewrap_keys(&config)?;
        }
        Command::RotateVapidKey { config } => {
            drop(out);
            rotate_vapid_key(&config)?;
        }
        Command::Discover(arguments) => {
            let found = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("runtime")?
                .block_on(discover::run(arguments))?;
            out.write_all(found.as_bytes())?;
        }
    }
    Ok(())
}

fn generate_master_key() -> anyhow::Result<String> {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|_| anyhow::anyhow!("random number generator failed"))?;
    Ok(BASE64.encode(&key))
}

fn hash_secret(out: &mut impl io::Write) -> anyhow::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let typed = input.trim_end_matches(['\r', '\n']);
    if typed.is_empty() {
        let secret = Token::generate()?;
        let hash = ClientSecretHash::hash(&SecretString::from(secret.expose()))?;
        writeln!(out, "secret: {}", secret.expose())?;
        writeln!(out, "hash:   {}", hash.as_str())?;
    } else {
        let hash = ClientSecretHash::hash(&SecretString::from(typed))?;
        writeln!(out, "{}", hash.as_str())?;
    }
    Ok(())
}

fn init_logs() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    // The panic message can hold mail data: log only where it happened.
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()));
        tracing::error!(location = location.as_deref().unwrap_or("unknown"), "panic");
    }));
}

/// The configuration file and the master keys.
fn load(path: &Path) -> anyhow::Result<(Config, Keyring)> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let env_key = std::env::var(MASTER_KEY_ENV).ok().map(SecretString::from);
    let config = Config::parse(&text, base, env_key.is_some())?;
    let keyring = config.keyring(env_key.as_ref())?;
    drop(env_key);
    Ok((config, keyring))
}

fn serve(path: &Path) -> anyhow::Result<()> {
    init_logs();
    let (config, keyring) = load(path)?;
    warn_about_upstream(&config);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_server(config, keyring))
}

fn warn_about_upstream(config: &Config) {
    if config.client_ip_warning {
        tracing::warn!(
            "client_ip = peer with an https public_url: every login shares the proxy's address \
             in the rate limits; one client can block all logins"
        );
    }
    for (what, server) in [
        ("imap", &config.upstream.imap),
        ("smtp", &config.upstream.smtp),
    ] {
        tracing::info!(server = ?server, "upstream {what}");
        if server.certificate == CertificateCheck::InsecureLoopbackOnly {
            tracing::warn!(
                "upstream {what}: TLS certificates are NOT checked (tls_verify = false)"
            );
        }
    }
}

fn rewrap_keys(path: &Path) -> anyhow::Result<()> {
    init_logs();
    let (config, keyring) = load(path)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let store = open_store(&config).await?;
            rewrap(&store, &keyring).await
        })
}

fn rotate_vapid_key(path: &Path) -> anyhow::Result<()> {
    init_logs();
    let (config, _keyring) = load(path)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let store = open_store(&config).await?;
            let (existed, subscriptions) = vapid::rotate(&store).await?;
            tracing::warn!(
                existed,
                subscriptions,
                "VAPID key deleted with every push subscription: a new key is made at the next \
                 start, and browsers subscribe again when the web app opens"
            );
            Ok(())
        })
}

async fn open_store(config: &Config) -> anyhow::Result<nebula_store::Store> {
    let store = nebula_store::Store::open(&config.store_path)
        .await
        .with_context(|| format!("cannot open the database {}", config.store_path.display()))?;
    warn_if_store_exposed(&config.store_path);
    Ok(store)
}

/// The database holds sealed secrets, token hashes and the mail metadata
/// cache in clear: new files are 0600, but older installs (or a copied
/// file) may not be.
fn warn_if_store_exposed(path: &Path) {
    let message = "the database is readable by group or others; use chmod 600";
    config::warn_if_exposed(path, 0o077, message);
    for suffix in ["-wal", "-shm"] {
        let mut companion = path.as_os_str().to_owned();
        companion.push(suffix);
        config::warn_if_exposed(Path::new(&companion), 0o077, message);
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        config::warn_if_exposed(
            parent,
            0o007,
            "the database directory is open to others; use chmod 700",
        );
    }
}

/// Re-wraps the data keys made with an older master key,
/// and says whether the older keys are still needed.
async fn rewrap(store: &nebula_store::Store, keyring: &Keyring) -> anyhow::Result<()> {
    let mut report = nebula_http::rewrap_data_keys(store, keyring)
        .await
        .context("cannot re-wrap the data keys")?;
    // The bridge's own key (Web Push), wrapped like the accounts'.
    match vapid::rewrap(store, keyring).await {
        Ok(true) => report.rewrapped = report.rewrapped.saturating_add(1),
        Ok(false) => {}
        Err(error) => {
            tracing::error!(error = %error, "the VAPID key could not be re-wrapped");
            report.failed = report.failed.saturating_add(1);
        }
    }
    let current = keyring.current_version().get();
    if report.rewrapped > 0 {
        tracing::info!(
            count = report.rewrapped,
            "data keys wrapped again with master key {current}"
        );
    }
    let previous = keyring.previous_versions();
    if report.failed > 0 {
        tracing::error!(
            count = report.failed,
            "data keys could not be unwrapped with the configured master keys: those accounts \
             must log in again"
        );
    } else if !previous.is_empty() {
        let list: Vec<String> = previous.iter().map(ToString::to_string).collect();
        tracing::warn!(
            "every data key uses master key {current}: the older keys ({}) are no longer \
             needed and can be removed from the configuration",
            list.join(", ")
        );
    }
    Ok(())
}

async fn run_server(config: Config, keyring: Keyring) -> anyhow::Result<()> {
    let store = open_store(&config).await?;
    // Automatic after a rotation: the old master key is needed only until
    // this has run once.
    rewrap(&store, &keyring).await?;
    let shutdown = CancellationToken::new();
    let vapid = match &config.push {
        Some(push) => Some((
            push.clone(),
            std::sync::Arc::new(vapid::load_or_create(&store, &keyring).await?),
        )),
        None => None,
    };
    let backend = backend::build(&config.upstream, &store, config.hold, vapid)?;
    let background = tokio::spawn({
        let backend = backend.clone();
        let shutdown = shutdown.clone();
        async move { backend.run(shutdown).await }
    });
    let server = nebula_http::Server::new(
        config.http,
        store.clone(),
        keyring,
        backend,
        shutdown.clone(),
    );
    let cleanup = tokio::spawn({
        let server = server.clone();
        async move { server.run_cleanup(CLEANUP_EVERY).await }
    });
    tokio::spawn(wait_for_signal(shutdown));
    // Accounts signed out with scheduled messages that go out anyway
    // (`[submission] send_while_signed_out`): running again now, so the
    // scheduler can send them.
    {
        let server = server.clone();
        tokio::spawn(async move {
            match server.accounts_held_while_signed_out().await {
                Ok(accounts) => server.start_accounts(&accounts).await,
                Err(error) => {
                    tracing::warn!(%error, "accounts with scheduled messages unavailable");
                }
            }
        });
    }
    // Accounts with Web Push: running again now, not at their next request,
    // so new mail and reminders reach closed tabs after a restart.
    if config.push.is_some() {
        let server = server.clone();
        let store = store.clone();
        tokio::spawn(async move {
            match nebula_store::UnixTime::now() {
                Ok(now) => match store.push_accounts(now).await {
                    Ok(accounts) => server.start_accounts(&accounts).await,
                    Err(error) => tracing::warn!(%error, "web push accounts unavailable"),
                },
                Err(error) => tracing::warn!(%error, "clock"),
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("cannot listen on {}", config.listen))?;
    tracing::info!(listen = %config.listen, "bridge listening");
    server.serve(listener).await?;
    cleanup.await?;
    background.await?;
    tracing::info!("bridge stopped");
    Ok(())
}

/// Cancels `shutdown` on Ctrl-C or SIGTERM.
async fn wait_for_signal(shutdown: CancellationToken) {
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
    shutdown.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn command_line() {
        assert_eq!(
            parse_args(&args(&["serve", "--config", "a.toml"])).unwrap(),
            Command::Serve {
                config: PathBuf::from("a.toml")
            }
        );
        assert_eq!(
            parse_args(&args(&["hash-secret"])).unwrap(),
            Command::HashSecret
        );
        assert_eq!(
            parse_args(&args(&["gen-master-key"])).unwrap(),
            Command::GenMasterKey
        );
        assert_eq!(
            parse_args(&args(&["rewrap-keys", "--config", "a.toml"])).unwrap(),
            Command::RewrapKeys {
                config: PathBuf::from("a.toml")
            }
        );
        assert!(parse_args(&args(&["rewrap-keys"])).is_err());
        assert_eq!(
            parse_args(&args(&["rotate-vapid-key", "--config", "a.toml"])).unwrap(),
            Command::RotateVapidKey {
                config: PathBuf::from("a.toml")
            }
        );
        assert_eq!(parse_args(&args(&[])).unwrap(), Command::Help);
        assert!(parse_args(&args(&["serve"])).is_err());
        assert!(parse_args(&args(&["serve", "--config"])).is_err());
        assert!(parse_args(&args(&["delete-everything"])).is_err());
    }

    #[test]
    fn master_keys_are_32_random_bytes() {
        let a = generate_master_key().unwrap();
        let b = generate_master_key().unwrap();
        assert_ne!(a, b);
        assert_eq!(BASE64.decode(a.as_bytes()).unwrap().len(), 32);
        let key = nebula_crypto::MasterKey::from_base64(
            nebula_crypto::KeyVersion::new(1).unwrap(),
            &SecretString::from(a),
        );
        assert!(key.is_ok());
    }
}
