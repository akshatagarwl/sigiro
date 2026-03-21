use anyhow::{Context, Result};
use clap::{Parser, Subcommand, builder::styling};
use duckdb::Connection;
use std::sync::{Arc, Mutex};

use sigiro::{data_dir, open_db, storage};

const STYLES: styling::Styles = styling::Styles::styled()
    .header(styling::AnsiColor::Cyan.on_default().bold())
    .usage(styling::AnsiColor::Cyan.on_default().bold())
    .literal(styling::AnsiColor::Green.on_default().bold())
    .placeholder(styling::AnsiColor::Yellow.on_default());

#[derive(Parser)]
#[command(name = "sigiro", about = "Observability for AI agents", styles = STYLES)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start gRPC + HTTP servers
    Serve,
    /// Run SQL query
    Query {
        /// SQL statement to execute
        sql: String,
    },
    /// Show server health
    Status,
    /// Investigate a service — returns structured metrics, spans, logs, linked traces
    Investigate {
        /// Service name to investigate
        service: String,
        /// Start of range in epoch seconds (defaults to now - 900)
        #[arg(long)]
        from: Option<i64>,
        /// End of range in epoch seconds (defaults to now)
        #[arg(long)]
        to: Option<i64>,
    },
    /// Start MCP stdio server for Claude/Cursor/Codex integration
    Mcp,
    /// Save API key for hosted mode
    Auth {
        /// API key (e.g. sigiro_...)
        key: String,
    },
    /// Admin commands for key management
    Admin {
        #[command(subcommand)]
        action: AdminAction,
    },
}

#[derive(Subcommand)]
enum AdminAction {
    /// Create a new API key for a project
    CreateKey {
        /// Project ID
        #[arg(long)]
        project: String,
        /// Plan (free, builder, team, pro)
        #[arg(long, default_value = "free")]
        plan: String,
        /// Rate limit (requests per minute)
        #[arg(long, default_value = "100")]
        rate_limit: u32,
    },
    /// Revoke an API key by short token
    RevokeKey {
        /// Short token portion of the key
        short_token: String,
    },
}

/// Open or create the admin DuckDB (for API key storage).
fn open_admin_db() -> Result<Connection> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("admin.duckdb");
    let conn = Connection::open(&path)
        .with_context(|| format!("failed to open admin DB at {}", path.display()))?;
    sigiro::auth::create_keys_table(&conn)?;
    Ok(conn)
}

/// Initialize DuckLake if configured, return the table prefix ("lake." or "").
/// Uses a 'static str so it can be shared across threads without lifetime issues.
fn init_storage(conn: &Connection) -> Result<&'static str> {
    match storage::load_config() {
        Some(config) => {
            storage::init_ducklake(conn, &config)?;
            tracing::info!("DuckLake mode — tables in lake.*");
            Ok(storage::table_prefix(true))
        }
        None => {
            tracing::info!("local mode — no SIGIRO_CATALOG_URL set");
            Ok(storage::table_prefix(false))
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sigiro=info".parse().expect("valid filter")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Command::Serve) | None => {
            let conn = open_db()?;
            let prefix = init_storage(&conn)?;
            sigiro::schema::create_tables(&conn, prefix)?;

            let admin_conn = open_admin_db()?;

            let db_path = data_dir()?.join("sigiro.duckdb");
            tracing::info!("sigiro ready — {}", db_path.display());

            let (tx, rx) = crossbeam_channel::unbounded();

            let shared_conn = Arc::new(Mutex::new(conn));
            let shared_admin = Arc::new(Mutex::new(admin_conn));

            let inserter_conn = shared_conn.clone();
            let inserter_prefix = prefix.to_string();
            std::thread::spawn(move || {
                if let Err(e) = sigiro::inserter::run_inserter(&inserter_conn, rx, &inserter_prefix)
                {
                    tracing::error!("inserter fatal: {e:#}");
                    std::process::exit(1);
                }
            });

            // Spawn compactor only in DuckLake mode (local DuckDB doesn't need it)
            if !prefix.is_empty() {
                let compactor_conn = shared_conn.clone();
                tokio::spawn(async move {
                    sigiro::compactor::run_compactor(compactor_conn, 7, prefix).await;
                });
            }

            sigiro::server::run_servers(shared_conn, shared_admin, tx, prefix).await?;
        }
        Some(Command::Query { sql }) => {
            let client = require_client()?;
            let result = client.query(&sql).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some(Command::Status) => {
            let client = require_client()?;
            let result = client.status().await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some(Command::Investigate { service, from, to }) => {
            let client = require_client()?;
            let block = client
                .investigate(
                    &service,
                    from.map(|s| s * 1_000_000),
                    to.map(|s| s * 1_000_000),
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&block)?);
        }
        Some(Command::Mcp) => {
            let conn = open_db()?;
            let prefix = init_storage(&conn)?;
            sigiro::schema::create_tables(&conn, prefix)?;
            let shared = Arc::new(Mutex::new(conn));
            sigiro::mcp::run_mcp_server(shared, prefix).await?;
        }
        Some(Command::Auth { key }) => {
            let path = sigiro::config::save_api_key(&key)?;
            println!("API key saved to {}", path.display());
        }
        Some(Command::Admin { action }) => {
            let admin_conn = open_admin_db()?;
            let admin = Mutex::new(admin_conn);
            match action {
                AdminAction::CreateKey {
                    project,
                    plan,
                    rate_limit,
                } => {
                    let raw_key = sigiro::auth::create_key(&admin, &project, &plan, rate_limit)?;
                    println!("{raw_key}");
                }
                AdminAction::RevokeKey { short_token } => {
                    if sigiro::auth::revoke_key(&admin, &short_token)? {
                        println!("Key revoked: {short_token}");
                    } else {
                        println!("No key found with short token: {short_token}");
                    }
                }
            }
        }
    }
    Ok(())
}

/// Build HTTP client from config. Fails fast if no API key is configured.
fn require_client() -> Result<sigiro::client::SigiroClient> {
    sigiro::client::try_build_client().context("No API key configured. Run: sigiro auth <key>")
}
