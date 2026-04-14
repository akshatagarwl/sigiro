use anyhow::{Context, Result};
use clap::{Parser, Subcommand, builder::styling};
use duckdb::Connection as DuckConnection;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

use sigiro::toon::to_toon;
use sigiro::{data_dir, inserter::MsgRouter, pool::ProjectPool};

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
    /// Output format (toon or json)
    #[arg(long, default_value = "toon")]
    output: OutputFormat,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum OutputFormat {
    /// Token-Oriented Object Notation (default, ~40% fewer tokens)
    Toon,
    /// Standard JSON format
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Start gRPC + HTTP servers
    Serve {
        /// Disable eBPF auto-instrumentation (run in OTLP-only mode)
        #[arg(long)]
        no_ebpf: bool,
    },
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

/// Open or create the admin SQLite database (for API key storage).
fn open_admin_db() -> Result<Connection> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("admin.sqlite");
    let conn = Connection::open(&path)
        .with_context(|| format!("failed to open admin DB at {}", path.display()))?;
    sigiro::admin::create_keys_table(&conn)?;
    Ok(conn)
}

/// Format output based on the selected output format.
fn format_output(value: &serde_json::Value, output_format: &OutputFormat) -> Result<String> {
    match output_format {
        OutputFormat::Toon => Ok(to_toon(value)),
        OutputFormat::Json => Ok(serde_json::to_string_pretty(value)?),
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
        Some(Command::Serve { no_ebpf }) => {
            run_serve(no_ebpf).await?;
        }
        None => {
            run_serve(false).await?;
        }
        Some(Command::Query { sql }) => {
            let client = require_client()?;
            let result = client.query(&sql).await?;
            println!(
                "{}",
                format_output(&serde_json::to_value(&result)?, &cli.output)?
            );
        }
        Some(Command::Status) => {
            let client = require_client()?;
            let result = client.status().await?;
            println!("{}", format_output(&result, &cli.output)?);
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
            println!(
                "{}",
                format_output(&serde_json::to_value(&block)?, &cli.output)?
            );
        }
        Some(Command::Mcp) => {
            let client = require_client()?;
            tracing::info!("MCP server connecting to {}", client.base_url());
            sigiro::mcp::run_mcp_server(client).await?;
        }
        Some(Command::Auth { key }) => {
            let path = sigiro::config::save_api_key(&key)?;
            println!("API key saved to {}", path.display());
        }
        Some(Command::Admin { action }) => {
            // Try HTTP to running server first (solves DuckDB single-writer lock).
            // Fall back to direct DuckDB if server isn't running (bootstrapping).
            let server_url = std::env::var("SIGIRO_ENDPOINT")
                .context("SIGIRO_ENDPOINT not set. Export SIGIRO_ENDPOINT=http://localhost:9999 (or your server URL)")?;

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
                                sigiro::admin::create_key(&admin, &project, &plan, rate_limit)?;
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
                            if sigiro::admin::revoke_key(&admin, &short_token)? {
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

async fn run_serve(no_ebpf: bool) -> Result<()> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;

    // DuckLake init is now per-project (in pool.get_or_create).
    // No need for a probe connection at startup.
    let pool = Arc::new(ProjectPool::new(dir.clone(), 100));
    let admin_conn = open_admin_db()?;

    // Write instance status at startup
    sigiro::admin::write_instance_status(&admin_conn, "version", env!("CARGO_PKG_VERSION"))?;
    sigiro::admin::write_instance_status(&admin_conn, "mode", "otlp")?;
    sigiro::admin::write_instance_status(&admin_conn, "platform_os", std::env::consts::OS)?;
    sigiro::admin::write_instance_status(&admin_conn, "platform_arch", std::env::consts::ARCH)?;

    // Probe fleet extensions and store result
    let probe = DuckConnection::open_in_memory()
        .context("failed to open probe DuckDB for extension check")?;
    let loaded = sigiro::schema::load_fleet_extensions(&probe);
    drop(probe);
    sigiro::admin::write_instance_status(
        &admin_conn,
        "extensions",
        &serde_json::to_string(&loaded).unwrap_or_else(|_| "[]".to_string()),
    )?;

    // eBPF auto-instrumentation: try obi, degrade gracefully
    if no_ebpf {
        tracing::info!("eBPF disabled by --no-ebpf flag");
        sigiro::admin::write_instance_status(&admin_conn, "ebpf_status", "disabled")?;
        sigiro::admin::write_instance_status(&admin_conn, "ebpf_reason", "--no-ebpf flag")?;
    } else {
        let otlp_endpoint = format!(
            "http://127.0.0.1:{}",
            std::env::var("SIGIRO_PORT").unwrap_or_else(|_| "9999".to_string())
        );
        let status = sigiro::ebpf::ebpf_status("obi");
        sigiro::ebpf::write_ebpf_status(&admin_conn, &status)?;
        match status.status.as_str() {
            "ready" => {
                sigiro::ebpf::start_obi("obi", &otlp_endpoint)?;
            }
            _ => {
                tracing::warn!(
                    "eBPF unavailable: {} — running in OTLP-only mode",
                    status.reason
                );
            }
        }
    }

    tracing::info!("sigiro ready — data_dir={}", dir.display());

    let shared_admin = Arc::new(Mutex::new(admin_conn));

    // MsgRouter: handlers send directly to per-project flusher tasks.
    // No main channel, no dispatcher thread.
    let router = Arc::new(MsgRouter::new(Arc::clone(&pool)));

    // Evict idle connections every 2 minutes
    let evictor_pool = Arc::clone(&pool);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(120));
        loop {
            interval.tick().await;
            evictor_pool.evict_idle(std::time::Duration::from_secs(300));
        }
    });

    sigiro::server::run_servers(pool, shared_admin, router).await?;
    Ok(())
}

/// Build HTTP client from config. Fails fast if API key or endpoint is not configured.
fn require_client() -> Result<sigiro::client::SigiroClient> {
    sigiro::client::try_build_client()
}
