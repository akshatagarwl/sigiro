mod ingest;
mod inserter;
mod investigate;
mod schema;
mod server;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use duckdb::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser)]
#[command(name = "sigiro", about = "Observability for AI agents")]
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
}

fn data_dir() -> Result<PathBuf> {
    match std::env::var("SIGIRO_DATA_DIR") {
        Ok(dir) => Ok(PathBuf::from(dir)),
        Err(_) => {
            let home = dirs::home_dir().context("cannot determine home directory")?;
            Ok(home.join(".sigiro").join("data"))
        }
    }
}

fn open_db() -> Result<Connection> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create data directory: {}", dir.display()))?;
    let db_path = dir.join("sigiro.duckdb");
    Connection::open(&db_path)
        .with_context(|| format!("failed to open DuckDB at {}", db_path.display()))
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
            schema::create_tables(&conn)?;

            let db_path = data_dir()?.join("sigiro.duckdb");
            tracing::info!("sigiro ready — {}", db_path.display());

            let (tx, rx) = crossbeam_channel::unbounded();

            // Single shared connection for both insert and query
            let shared_conn = Arc::new(Mutex::new(conn));

            // Inserter runs on a dedicated OS thread
            let inserter_conn = shared_conn.clone();
            std::thread::spawn(move || {
                if let Err(e) = inserter::run_inserter(inserter_conn, rx) {
                    tracing::error!("inserter fatal: {e:#}");
                    std::process::exit(1);
                }
            });

            server::run_servers(shared_conn, tx).await?;
        }
        Some(Command::Query { sql }) => {
            let conn = open_db()?;
            run_query(&conn, &sql)?;
        }
        Some(Command::Status) => {
            let conn = open_db()?;
            schema::create_tables(&conn)?;
            print_status(&conn)?;
        }
        Some(Command::Investigate { service, window }) => {
            let conn = open_db()?;
            schema::create_tables(&conn)?;
            let shared = Arc::new(Mutex::new(conn));
            let window_us = parse_window(&window)?;
            let now_us = chrono::Utc::now().timestamp_micros();
            let req = investigate::InvestigateRequest {
                service,
                from_ts: Some(now_us - window_us),
                to_ts: Some(now_us),
            };
            let block = investigate::investigate(&shared, &req)?;
            println!("{}", serde_json::to_string_pretty(&block)?);
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

fn parse_window(s: &str) -> Result<i64> {
    let s = s.trim();
    if let Some(m) = s.strip_suffix('m') {
        Ok(m.parse::<i64>().context("invalid minutes")? * 60 * 1_000_000)
    } else if let Some(h) = s.strip_suffix('h') {
        Ok(h.parse::<i64>().context("invalid hours")? * 3600 * 1_000_000)
    } else {
        anyhow::bail!("window must end with 'm' or 'h' (e.g. 15m, 1h)")
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

    // DB file size
    let db_path = data_dir()?.join("sigiro.duckdb");
    let db_size = std::fs::metadata(&db_path)
        .map(|m| format_bytes(m.len()))
        .unwrap_or_else(|_| "N/A".to_string());

    println!("sigiro status");
    println!("─────────────");
    println!("  db: {} ({})", db_path.display(), db_size);
    println!();

    // Services seen
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

    // Row counts per table
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_window_minutes() {
        assert_eq!(parse_window("15m").unwrap(), 15 * 60 * 1_000_000);
        assert_eq!(parse_window("1m").unwrap(), 60 * 1_000_000);
    }

    #[test]
    fn parse_window_hours() {
        assert_eq!(parse_window("1h").unwrap(), 3600 * 1_000_000);
        assert_eq!(parse_window("2h").unwrap(), 2 * 3600 * 1_000_000);
    }

    #[test]
    fn parse_window_trims() {
        assert_eq!(parse_window("  15m  ").unwrap(), 15 * 60 * 1_000_000);
    }

    #[test]
    fn parse_window_rejects_invalid() {
        assert!(parse_window("15").is_err());
        assert!(parse_window("abc").is_err());
        assert!(parse_window("15d").is_err());
    }

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn format_bytes_fractional() {
        assert_eq!(format_bytes(1536), "1.5 KB");
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
