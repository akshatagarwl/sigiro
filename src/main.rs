use anyhow::{Context, Result};
use clap::{Parser, Subcommand, builder::styling};
use duckdb::Connection;
use std::sync::{Arc, Mutex};

use sigiro::{data_dir, format_bytes, open_db, parse_window};

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
        /// Time window (e.g. 15m, 1h)
        #[arg(long, default_value = "15m")]
        window: String,
    },
    /// Start MCP stdio server for Claude/Cursor/Codex integration
    Mcp,
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
            sigiro::schema::create_tables(&conn)?;

            let db_path = data_dir()?.join("sigiro.duckdb");
            tracing::info!("sigiro ready — {}", db_path.display());

            let (tx, rx) = crossbeam_channel::unbounded();

            let shared_conn = Arc::new(Mutex::new(conn));

            let inserter_conn = shared_conn.clone();
            std::thread::spawn(move || {
                if let Err(e) = sigiro::inserter::run_inserter(inserter_conn, rx) {
                    tracing::error!("inserter fatal: {e:#}");
                    std::process::exit(1);
                }
            });

            sigiro::server::run_servers(shared_conn, tx).await?;
        }
        Some(Command::Query { sql }) => {
            let conn = open_db()?;
            run_query(&conn, &sql)?;
        }
        Some(Command::Status) => {
            let conn = open_db()?;
            sigiro::schema::create_tables(&conn)?;
            print_status(&conn)?;
        }
        Some(Command::Investigate { service, window }) => {
            let conn = open_db()?;
            sigiro::schema::create_tables(&conn)?;
            let shared = Arc::new(Mutex::new(conn));
            let window_us = parse_window(&window)?;
            let now_us = chrono::Utc::now().timestamp_micros();
            let req = sigiro::investigate::InvestigateRequest {
                service,
                from_ts: Some(now_us - window_us),
                to_ts: Some(now_us),
            };
            let block = sigiro::investigate::investigate(&shared, &req)?;
            println!("{}", serde_json::to_string_pretty(&block)?);
        }
        Some(Command::Mcp) => {
            let conn = open_db()?;
            sigiro::schema::create_tables(&conn)?;
            let shared = Arc::new(Mutex::new(conn));
            sigiro::mcp::run_mcp_server(shared).await?;
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
                .map(|s| s.to_string())
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

fn print_status(conn: &Connection) -> Result<()> {
    let tables = [
        "sigiro_spans",
        "sigiro_logs",
        "sigiro_metrics_gauge",
        "sigiro_metrics_sum",
        "sigiro_metrics_histogram",
        "sigiro_metrics_exp_histogram",
    ];

    let db_path = data_dir()?.join("sigiro.duckdb");
    let db_size = std::fs::metadata(&db_path)
        .map(|m| format_bytes(m.len()))
        .unwrap_or_else(|_| "N/A".to_string());

    println!("sigiro status");
    println!("─────────────");
    println!("  db: {} ({})", db_path.display(), db_size);
    println!();

    match conn.prepare("SELECT DISTINCT service_name FROM sigiro_spans UNION SELECT DISTINCT service_name FROM sigiro_logs UNION SELECT DISTINCT service_name FROM sigiro_metrics_gauge") {
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

    for table in tables {
        match conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        }) {
            Ok(count) => println!("  {table}: {count} rows"),
            Err(e) => tracing::error!("{table}: {e}"),
        }
    }
    Ok(())
}
