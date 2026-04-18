use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;

/// Signal types that map to Lago billable metrics.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Signal {
    Spans,
    Logs,
    Metrics,
    Queries,
}

impl Signal {
    pub fn lago_code(&self) -> &'static str {
        match self {
            Signal::Spans => "spans_ingested",
            Signal::Logs => "logs_ingested",
            Signal::Metrics => "metrics_ingested",
            Signal::Queries => "queries_executed",
        }
    }
}

/// Thread-safe usage counters keyed by (project_id, signal).
pub struct UsageCounters {
    inner: DashMap<(String, Signal), AtomicU64>,
}

impl Default for UsageCounters {
    fn default() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }
}

impl UsageCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment usage for a project + signal.
    pub fn increment(&self, project_id: &str, signal: Signal, count: u64) {
        self.inner
            .entry((project_id.to_string(), signal))
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(count, Ordering::Relaxed);
    }

    /// Drain all counters, returning (project_id, signal, count) triples.
    /// Resets all counters to 0.
    pub fn drain(&self) -> Vec<(String, Signal, u64)> {
        let mut result = Vec::new();
        for entry in self.inner.iter() {
            let count = entry.value().swap(0, Ordering::Relaxed);
            if count > 0 {
                let (project_id, signal) = entry.key();
                result.push((project_id.clone(), *signal, count));
            }
        }
        result
    }
}

/// Build Lago batch event JSON from drained counters.
pub fn build_lago_events(
    counters: &[(String, Signal, u64)],
    transaction_id_prefix: &str,
) -> Vec<serde_json::Value> {
    counters
        .iter()
        .map(|(project_id, signal, count)| {
            serde_json::json!({
                "transaction_id": format!("{transaction_id_prefix}_{project_id}_{}", signal.lago_code()),
                "external_subscription_id": project_id,
                "code": signal.lago_code(),
                "properties": {
                    "count": count,
                },
            })
        })
        .collect()
}

/// Chunk events into batches of at most `chunk_size` for Lago's API limit.
pub fn chunk_events(
    events: &[serde_json::Value],
    chunk_size: usize,
) -> Vec<Vec<serde_json::Value>> {
    events
        .chunks(chunk_size)
        .map(<[serde_json::Value]>::to_vec)
        .collect()
}
