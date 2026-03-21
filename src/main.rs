use anyhow::{Context, Result};
use clap::{Parser, Subcommand, builder::styling};
use duckdb::Connection;
use std::sync::{Arc, Mutex};

use sigiro::{data_dir, format_bytes, open_db, storage};

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
    /// Run SQL against local DuckDB
    Query {
        /// SQL statement to execute
        sql: String,
    },
    /// Show services seen, row counts, data size, uptime
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
        /// API key (e.g. sk_live_...)
        key: String,
    },
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

            let db_path = data_dir()?.join("sigiro.duckdb");
            tracing::info!("sigiro ready — {}", db_path.display());

            let (tx, rx) = crossbeam_channel::unbounded();

            let shared_conn = Arc::new(Mutex::new(conn));

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

            sigiro::server::run_servers(shared_conn, tx, prefix).await?;
        }
        Some(Command::Query { sql }) => {
            let conn = open_db()?;
            let prefix = init_storage(&conn)?;
            sigiro::schema::create_tables(&conn, prefix)?;
            run_query(&conn, &sql)?;
        }
        Some(Command::Status) => {
            let conn = open_db()?;
            let prefix = init_storage(&conn)?;
            sigiro::schema::create_tables(&conn, prefix)?;
            print_status(&conn, prefix)?;
        }
        Some(Command::Investigate { service, from, to }) => {
            let conn = open_db()?;
            let prefix = init_storage(&conn)?;
            sigiro::schema::create_tables(&conn, prefix)?;
            let shared = Arc::new(Mutex::new(conn));
            let req = sigiro::investigate::InvestigateRequest {
                service,
                from_ts: from.map(|s| s * 1_000_000),
                to_ts: to.map(|s| s * 1_000_000),
            };
            let block = sigiro::investigate::investigate(&shared, &req, prefix)?;
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
    }
    Ok(())
}

fn run_query(conn: &Connection, sql: &str) -> Result<()> {
    let mut stmt = conn.prepare(sql).context("failed to prepare SQL")?;
    let mut rows = stmt.query([]).context("failed to execute query")?;

    let stmt_ref = rows.as_ref().context("no result set")?;
    let column_count = stmt_ref.column_count();
    let column_names: Vec<String> = (0..column_count)
        .map(|i| {
            stmt_ref
                .column_name(i)
                .map(ToString::to_string)
                .unwrap_or_else(|_| "?".to_string())
        })
        .collect();
    println!("{}", column_names.join("\t"));

    while let Some(row) = rows.next()? {
        let vals: Vec<String> = (0..column_count)
            .map(|i| value_ref_to_string(row.get_ref(i).expect("valid column index")))
            .collect();
        println!("{}", vals.join("\t"));
    }
    Ok(())
}

/// Convert a DuckDB ValueRef to its string representation for CLI tabular output.
fn value_ref_to_string(val: duckdb::types::ValueRef<'_>) -> String {
    use duckdb::types::ValueRef;
    match val {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Boolean(b) => b.to_string(),
        ValueRef::TinyInt(n) => n.to_string(),
        ValueRef::SmallInt(n) => n.to_string(),
        ValueRef::Int(n) => n.to_string(),
        ValueRef::BigInt(n) => n.to_string(),
        ValueRef::HugeInt(n) => n.to_string(),
        ValueRef::UTinyInt(n) => n.to_string(),
        ValueRef::USmallInt(n) => n.to_string(),
        ValueRef::UInt(n) => n.to_string(),
        ValueRef::UBigInt(n) => n.to_string(),
        ValueRef::Float(f) => f.to_string(),
        ValueRef::Double(f) => f.to_string(),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .unwrap_or("<invalid utf8>")
            .to_string(),
        ValueRef::Blob(bytes) => format!("<blob {} bytes>", bytes.len()),
        ValueRef::Timestamp(_, us) => us.to_string(),
        ValueRef::Date32(d) => d.to_string(),
        ValueRef::Time64(_, t) => t.to_string(),
        _ => format!("{val:?}"),
    }
}

fn print_status(conn: &Connection, prefix: &str) -> Result<()> {
    let db_path = data_dir()?.join("sigiro.duckdb");
    let db_size = std::fs::metadata(&db_path)
        .map(|m| format_bytes(m.len()))
        .unwrap_or_else(|_| "N/A".to_string());

    println!("sigiro status");
    println!("─────────────");
    println!("  db: {} ({})", db_path.display(), db_size);
    if !prefix.is_empty() {
        println!("  mode: DuckLake (tables in lake.*)");
    } else {
        println!("  mode: local");
    }
    println!();

    match conn.prepare(&format!(
        "SELECT DISTINCT service_name FROM {prefix}sigiro_spans UNION SELECT DISTINCT service_name FROM {prefix}sigiro_logs UNION SELECT DISTINCT service_name FROM {prefix}sigiro_metrics_gauge"
    )) {
        Ok(mut stmt) => {
            let mut rows = stmt.query([])?;
            let mut services = Vec::new();
            while let Some(row) = rows.next()? {
                if let Ok(s) = row.get::<_, String>(0) {
                    services.push(s);
                }
            }
            if services.is_empty() {
                println!("  services: (none)");
            } else {
                println!("  services: {}", services.join(", "));
            }
        }
        Err(_) => println!("  services: (none)"),
    }
    println!();

    for table in storage::TABLE_NAMES {
        let qualified = format!("{prefix}{table}");
        match conn.query_row(&format!("SELECT count(*) FROM {qualified}"), [], |row| {
            row.get::<_, i64>(0)
        }) {
            Ok(count) => println!("  {qualified}: {count} rows"),
            Err(e) => tracing::error!("{qualified}: {e}"),
        }
    }
    Ok(())
}
