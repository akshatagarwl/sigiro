use duckdb::Connection;
use std::sync::{Arc, Mutex};

use crate::storage;

/// Run the DuckLake compaction + retention loop.
/// Executes hourly: merges small Parquet files, deletes expired data, cleans up old files.
///
/// TRADEOFF: Each table's 3 operations (merge, delete, cleanup) hold the mutex briefly.
/// The mutex is released between tables so the inserter can make progress.
/// Acceptable at 44GB/mo — DuckLake merge is typically <5s per table.
pub async fn run_compactor(
    conn: Arc<Mutex<Connection>>,
    retention_days: u32,
    prefix: &'static str,
) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600));
    interval.tick().await; // first tick is immediate, skip it

    loop {
        interval.tick().await;
        tracing::info!("compactor: starting hourly cycle");

        for table in storage::TABLE_NAMES {
            let qualified = format!("{prefix}{table}");
            // Per-table lock acquire/release — inserter can progress between tables
            if let Err(e) = compact_table(&conn, &qualified, retention_days) {
                tracing::error!("compactor: {qualified}: {e:#}");
            }
        }

        tracing::info!("compactor: hourly cycle complete");
    }
}

fn compact_table(
    conn: &Arc<Mutex<Connection>>,
    qualified: &str,
    retention_days: u32,
) -> anyhow::Result<()> {
    let conn = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;

    // Compaction: merge small adjacent Parquet files into larger ones.
    // Bug #866: ducklake_merge_adjacent_files may fail with config issues.
    // Non-fatal — log and continue. Files accumulate but queries still work.
    match conn.execute_batch(&format!(
        "CALL ducklake_merge_adjacent_files('{qualified}')"
    )) {
        Ok(()) => tracing::info!("compactor: merged files for {qualified}"),
        Err(e) => tracing::error!("compactor: merge failed for {qualified} (bug #866?): {e}"),
    }

    // Retention: delete data older than retention window.
    match conn.execute_batch(&format!(
        "DELETE FROM {qualified} WHERE timestamp < now()::TIMESTAMP - INTERVAL '{retention_days} DAY'"
    )) {
        Ok(()) => tracing::info!("compactor: retention DELETE for {qualified}"),
        Err(e) => tracing::error!("compactor: retention DELETE failed for {qualified}: {e}"),
    }

    // Cleanup: remove Parquet files that are no longer referenced after DELETE.
    match conn.execute_batch(&format!("CALL ducklake_cleanup_old_files('{qualified}')")) {
        Ok(()) => tracing::info!("compactor: cleaned up old files for {qualified}"),
        Err(e) => tracing::error!("compactor: cleanup failed for {qualified}: {e}"),
    }

    Ok(())
}
