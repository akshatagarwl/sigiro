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

/// Flush usage to Lago. Fire-and-forget: logs errors, never panics.
pub async fn flush_to_lago(
    client: &reqwest::Client,
    lago_url: &str,
    lago_api_key: &str,
    counters: &UsageCounters,
) {
    let drained = counters.drain();
    if drained.is_empty() {
        return;
    }

    let ts = chrono::Utc::now().timestamp();
    let events = build_lago_events(&drained, &format!("flush_{ts}"));
    let chunks = chunk_events(&events, 100);

    for chunk in chunks {
        let body = serde_json::json!({ "events": chunk });
        match client
            .post(format!("{lago_url}/api/v1/events/batch"))
            .bearer_auth(lago_api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if !resp.status().is_success() => {
                tracing::error!("Lago flush failed: HTTP {}", resp.status());
            }
            Err(e) => {
                tracing::error!("Lago flush failed: {e}");
            }
            Ok(_) => {
                tracing::info!("flushed {} usage events to Lago", chunk.len());
            }
        }
    }
}

/// Spawn a background task that flushes usage to Lago every `interval_secs`.
pub fn spawn_flush_task(
    client: reqwest::Client,
    lago_url: String,
    lago_api_key: String,
    counters: std::sync::Arc<UsageCounters>,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            flush_to_lago(&client, &lago_url, &lago_api_key, &counters).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_counter_increment() {
        let counters = UsageCounters::new();
        counters.increment("proj_1", Signal::Spans, 10);
        counters.increment("proj_1", Signal::Spans, 5);
        let drained = counters.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].2, 15);
    }

    #[test]
    fn usage_counter_drain_resets() {
        let counters = UsageCounters::new();
        counters.increment("proj_1", Signal::Logs, 3);
        let first = counters.drain();
        assert_eq!(first.len(), 1);
        let second = counters.drain();
        assert!(second.is_empty());
    }

    #[test]
    fn lago_event_format() {
        let events = build_lago_events(&[("proj_1".to_string(), Signal::Spans, 42)], "test_prefix");
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e["code"], "spans_ingested");
        assert_eq!(e["properties"]["count"], 42);
        assert!(
            e["transaction_id"]
                .as_str()
                .unwrap()
                .starts_with("test_prefix_proj_1_")
        );
    }

    #[test]
    fn lago_batch_chunking() {
        let events: Vec<serde_json::Value> =
            (0..250).map(|i| serde_json::json!({ "id": i })).collect();
        let chunks = chunk_events(&events, 100);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 100);
        assert_eq!(chunks[1].len(), 100);
        assert_eq!(chunks[2].len(), 50);
    }

    #[test]
    fn flush_empty_is_noop() {
        let counters = UsageCounters::new();
        let drained = counters.drain();
        assert!(drained.is_empty());
    }

    #[test]
    fn signal_lago_codes() {
        assert_eq!(Signal::Spans.lago_code(), "spans_ingested");
        assert_eq!(Signal::Logs.lago_code(), "logs_ingested");
        assert_eq!(Signal::Metrics.lago_code(), "metrics_ingested");
        assert_eq!(Signal::Queries.lago_code(), "queries_executed");
    }

    #[test]
    fn multiple_projects_and_signals() {
        let counters = UsageCounters::new();
        counters.increment("proj_1", Signal::Spans, 10);
        counters.increment("proj_2", Signal::Logs, 20);
        counters.increment("proj_1", Signal::Metrics, 5);
        let mut drained = counters.drain();
        drained.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.lago_code().cmp(b.1.lago_code())));
        assert_eq!(drained.len(), 3);
    }
}
