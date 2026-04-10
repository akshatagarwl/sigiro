use anyhow::{Context, Result};
use clap::{Parser, Subcommand, builder::styling};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

use sigiro::{data_dir, pool::ProjectPool, storage};

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
            let dir = data_dir()?;
            std::fs::create_dir_all(&dir)?;

            // DuckLake init is now per-project (in pool.get_or_create).
            // No need for a probe connection at startup.
            let pool = Arc::new(ProjectPool::new(dir.clone(), 100));
            let admin_conn = open_admin_db()?;

            tracing::info!("sigiro ready — data_dir={}", dir.display());

            let (tx, rx) = crossbeam_channel::unbounded();
            let shared_admin = Arc::new(Mutex::new(admin_conn));

            // Inserter thread: routes messages to per-project DuckDB via pool.
            // Each IngestMsg carries project_id from OTLP auth (Bearer token).
            let inserter_pool = Arc::clone(&pool);
            std::thread::spawn(move || {
                if let Err(e) = sigiro::inserter::run_inserter(&inserter_pool, rx) {
                    tracing::error!("inserter fatal: {e:#}");
                    std::process::exit(1);
                }
            });

            // Evict idle connections every 2 minutes
            let evictor_pool = Arc::clone(&pool);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(120));
                loop {
                    interval.tick().await;
                    evictor_pool.evict_idle(std::time::Duration::from_secs(300));
                }
            });

            sigiro::server::run_servers(pool, shared_admin, tx).await?;
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
            let conn = Connection::open_in_memory().context("failed to open in-memory DuckDB")?;
            let prefix = storage::table_prefix();
            let data_dir = data_dir()?;
            match storage::load_config() {
                Some(config) => {
                    storage::init_ducklake(&conn, &config)?;
                    tracing::info!("DuckLake distributed mode — catalog: PostgreSQL, data: S3");
                }
                None => {
                    storage::init_default(&conn, &data_dir)?;
                }
            }
            sigiro::schema::create_tables(&conn, prefix)?;
            let shared = Arc::new(Mutex::new(conn));
            sigiro::mcp::run_mcp_server(shared, prefix).await?;
        }
        Some(Command::Auth { key }) => {
            let path = sigiro::config::save_api_key(&key)?;
            println!("API key saved to {}", path.display());
        }
        Some(Command::Admin { action }) => {
            // Try HTTP to running server first (solves DuckDB single-writer lock).
            // Fall back to direct DuckDB if server isn't running (bootstrapping).
            let server_url = std::env::var("SIGIRO_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:9999".to_string());

            match action {
                AdminAction::CreateKey {
                    project,
                    plan,
                    rate_limit,
                } => {
                    let client = sigiro::client::SigiroClient::new(&server_url, "");
                    match client.admin_create_key(&project, &plan, rate_limit).await {
                        Ok(raw_key) => {
                            println!("{raw_key}");
                        }
                        Err(http_err) => {
                            tracing::debug!(
                                "server not reachable, falling back to direct DB: {http_err:#}"
                            );
                            let admin_conn = open_admin_db()?;
                            let admin = Mutex::new(admin_conn);
                            let raw_key =
                                sigiro::auth::create_key(&admin, &project, &plan, rate_limit)?;
                            println!("{raw_key}");
                        }
                    }
                }
                AdminAction::RevokeKey { short_token } => {
                    let client = sigiro::client::SigiroClient::new(&server_url, "");
                    match client.admin_revoke_key(&short_token).await {
                        Ok(true) => {
                            println!("Key revoked: {short_token}");
                        }
                        Ok(false) => {
                            println!("No key found with short token: {short_token}");
                        }
                        Err(http_err) => {
                            tracing::debug!(
                                "server not reachable, falling back to direct DB: {http_err:#}"
                            );
                            let admin_conn = open_admin_db()?;
                            let admin = Mutex::new(admin_conn);
                            if sigiro::auth::revoke_key(&admin, &short_token)? {
                                println!("Key revoked: {short_token}");
                            } else {
                                println!("No key found with short token: {short_token}");
                            }
                        }
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
