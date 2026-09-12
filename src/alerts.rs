use anyhow::{Context, Result};
use arrow::array::{Array, StringArray, TimestampMicrosecondArray};
use arrow::record_batch::RecordBatch;
use parking_lot::Mutex;
use prefixed_api_key::sha2::{Digest, Sha256};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub(crate) const OOM_FIRING_EVENT: &str = "container.oom_killed";
pub(crate) const OOM_RECOVERED_EVENT: &str = "container.oom_recovered";
const OOM_RULE_ID: &str = "container-oom";

/// Non-empty bearer credential used only for alert webhook delivery.
///
/// Deliberately omits `Debug`, `Display`, and Serde traits so configuration
/// logging and generic serialization cannot disclose the credential.
#[nutype::nutype(validate(not_empty), derive(Clone, AsRef))]
pub struct AlertWebhookToken(String);

#[derive(Clone)]
pub struct AlertStore {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
pub struct AlertEvent {
    pub(crate) project_id: String,
    pub(crate) event_name: String,
    pub(crate) timestamp_us: i64,
    pub(crate) service_name: String,
    pub(crate) container_name: String,
    pub(crate) container_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AlertInstance {
    pub(crate) fingerprint: String,
    pub(crate) rule_id: String,
    pub(crate) status: String,
    pub(crate) service_name: String,
    pub(crate) container_name: String,
    pub(crate) starts_at: String,
    pub(crate) ends_at: Option<String>,
    pub(crate) occurrence_count: u64,
}

#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub(crate) id: i64,
    pub(crate) payload: String,
}

#[derive(Clone)]
pub struct DeliveryConfig {
    url: url::Url,
    token: Option<AlertWebhookToken>,
}

impl fmt::Debug for DeliveryConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeliveryConfig")
            .field("configured", &true)
            .finish_non_exhaustive()
    }
}

impl DeliveryConfig {
    pub fn from_env() -> Result<Option<Self>> {
        let raw_url = crate::config::ALERT_WEBHOOK_URL.value()?;
        if raw_url.is_empty() {
            return Ok(None);
        }
        let url = url::Url::parse(&raw_url).context("SIGIRO_ALERT_WEBHOOK_URL is not a URL")?;
        anyhow::ensure!(
            url.username().is_empty() && url.password().is_none(),
            "SIGIRO_ALERT_WEBHOOK_URL must not contain credentials"
        );
        let loopback_http = url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        anyhow::ensure!(
            url.scheme() == "https" || loopback_http,
            "SIGIRO_ALERT_WEBHOOK_URL must use HTTPS unless it targets loopback"
        );
        let token = crate::config::alert_webhook_token()?;
        Ok(Some(Self { url, token }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessResult {
    pub(crate) accepted_events: usize,
    pub(crate) notifications_enqueued: usize,
}

#[derive(Debug)]
struct StoredAlert {
    status: String,
    service_name: String,
    container_name: String,
    starts_at_us: i64,
    ends_at_us: Option<i64>,
    occurrence_count: u64,
}

struct AlertTransition<'a> {
    event: &'a AlertEvent,
    fingerprint: &'a str,
    status: &'static str,
    starts_at_us: i64,
    ends_at_us: Option<i64>,
    occurrence_count: u64,
}

impl AlertStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self> {
        create_tables(&conn.lock())?;
        Ok(Self { conn })
    }

    pub(crate) fn admin_connection(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    pub(crate) fn process_log_batch(
        &self,
        project_id: &str,
        batch: &RecordBatch,
    ) -> Result<ProcessResult> {
        let events = events_from_log_batch(project_id, batch)?;
        self.process_events(&events)
    }

    pub(crate) fn process_events(&self, events: &[AlertEvent]) -> Result<ProcessResult> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .context("failed to start alert transaction")?;
        let mut accepted_events = 0;
        let mut notifications_enqueued = 0;

        for event in events {
            if event.event_name != OOM_FIRING_EVENT && event.event_name != OOM_RECOVERED_EVENT {
                continue;
            }
            let fingerprint = alert_fingerprint(event);
            let occurrence = occurrence_key(event);
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO alert_occurrences
                 (project_id, occurrence_key, fingerprint, observed_at_us)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    event.project_id,
                    occurrence,
                    fingerprint,
                    event.timestamp_us
                ],
            )?;
            if inserted == 0 {
                continue;
            }
            accepted_events += 1;

            let stored = read_stored_alert(&tx, &event.project_id, &fingerprint)?;
            if event.event_name == OOM_FIRING_EVENT {
                notifications_enqueued +=
                    process_firing(&tx, event, &fingerprint, stored.as_ref())?;
            } else if let Some(stored) = stored.as_ref()
                && stored.status == "firing"
                && event.timestamp_us >= stored.starts_at_us
            {
                process_resolution(&tx, event, &fingerprint, stored)?;
                notifications_enqueued += 1;
            }
        }

        tx.commit().context("failed to commit alert transaction")?;
        Ok(ProcessResult {
            accepted_events,
            notifications_enqueued,
        })
    }

    pub(crate) fn list(
        &self,
        project_id: &str,
        status: Option<&str>,
    ) -> Result<Vec<AlertInstance>> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT fingerprint, rule_id, status, service_name, container_name,
                    starts_at_us, ends_at_us, occurrence_count
             FROM alert_instances WHERE project_id = ?1",
        );
        if status.is_some() {
            sql.push_str(" AND status = ?2");
        }
        sql.push_str(" ORDER BY updated_at_us DESC");
        let mut stmt = conn.prepare(&sql)?;
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, u64>(7)?,
            ))
        };
        let rows = match status {
            Some(status) => stmt.query_map(params![project_id, status], map_row)?,
            None => stmt.query_map(params![project_id], map_row)?,
        };
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(
                |(
                    fingerprint,
                    rule_id,
                    status,
                    service_name,
                    container_name,
                    starts_at_us,
                    ends_at_us,
                    occurrence_count,
                )| {
                    Ok(AlertInstance {
                        fingerprint,
                        rule_id,
                        status,
                        service_name,
                        container_name,
                        starts_at: format_timestamp(starts_at_us)?,
                        ends_at: ends_at_us.map(format_timestamp).transpose()?,
                        occurrence_count,
                    })
                },
            )
            .collect()
    }

    pub(crate) fn next_due(&self, now_us: i64) -> Result<Option<PendingNotification>> {
        self.conn
            .lock()
            .query_row(
                "SELECT id, payload_json FROM alert_outbox
                 WHERE sent_at_us IS NULL AND next_attempt_at_us <= ?1
                 ORDER BY id LIMIT 1",
                [now_us],
                |row| {
                    Ok(PendingNotification {
                        id: row.get(0)?,
                        payload: row.get(1)?,
                    })
                },
            )
            .optional()
            .context("failed to read alert outbox")
    }

    pub(crate) fn mark_sent(&self, id: i64, sent_at_us: i64) -> Result<()> {
        let changed = self.conn.lock().execute(
            "UPDATE alert_outbox SET sent_at_us = ?2, last_error = NULL WHERE id = ?1",
            params![id, sent_at_us],
        )?;
        anyhow::ensure!(changed == 1, "alert outbox row {id} does not exist");
        Ok(())
    }

    pub(crate) fn mark_retry(&self, id: i64, now_us: i64, error_class: &str) -> Result<()> {
        anyhow::ensure!(
            error_class.len() <= 64
                && error_class
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
            "alert error class must be a short identifier"
        );
        let attempts: u32 = self.conn.lock().query_row(
            "SELECT attempts FROM alert_outbox WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let exponent = attempts.min(10);
        let delay_secs = 5_i64.saturating_mul(1_i64 << exponent).min(3_600);
        let changed = self.conn.lock().execute(
            "UPDATE alert_outbox
             SET attempts = attempts + 1, next_attempt_at_us = ?2, last_error = ?3
             WHERE id = ?1 AND sent_at_us IS NULL",
            params![id, now_us + delay_secs * 1_000_000, error_class],
        )?;
        anyhow::ensure!(changed == 1, "pending alert outbox row {id} does not exist");
        Ok(())
    }
}

/// Deliver persisted notifications. A crash after HTTP success and before
/// `mark_sent` can repeat a request, so receivers must deduplicate by fingerprint.
pub async fn run_delivery(store: AlertStore, config: DeliveryConfig) {
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!("failed to build alert webhook client: {error}");
            return;
        }
    };
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        loop {
            let notification = match store.next_due(now_us()) {
                Ok(Some(notification)) => notification,
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!("failed to read alert outbox: {error:#}");
                    break;
                }
            };
            let mut request = client
                .post(config.url.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(notification.payload);
            if let Some(token) = &config.token {
                request = request.bearer_auth(token.as_ref());
            }
            match request.send().await {
                Ok(response) if response.status().is_success() => {
                    if let Err(error) = store.mark_sent(notification.id, now_us()) {
                        tracing::warn!(
                            notification_id = notification.id,
                            "webhook accepted alert but sent state was not persisted: {error:#}"
                        );
                        break;
                    }
                }
                Ok(response) => {
                    let class = format!("http_{}", response.status().as_u16());
                    if let Err(error) = store.mark_retry(notification.id, now_us(), &class) {
                        tracing::warn!(
                            notification_id = notification.id,
                            "failed to schedule alert retry: {error:#}"
                        );
                        break;
                    }
                }
                Err(error) => {
                    let class = if error.is_timeout() {
                        "timeout"
                    } else if error.is_connect() {
                        "connect"
                    } else {
                        "request"
                    };
                    if let Err(error) = store.mark_retry(notification.id, now_us(), class) {
                        tracing::warn!(
                            notification_id = notification.id,
                            "failed to schedule alert retry: {error:#}"
                        );
                        break;
                    }
                }
            }
        }
    }
}

pub(crate) fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS alert_instances (
            project_id TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            rule_id TEXT NOT NULL,
            status TEXT NOT NULL,
            service_name TEXT NOT NULL,
            container_name TEXT NOT NULL,
            starts_at_us INTEGER NOT NULL,
            ends_at_us INTEGER,
            occurrence_count INTEGER NOT NULL,
            updated_at_us INTEGER NOT NULL,
            PRIMARY KEY (project_id, fingerprint)
         );
         CREATE TABLE IF NOT EXISTS alert_occurrences (
            project_id TEXT NOT NULL,
            occurrence_key TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            observed_at_us INTEGER NOT NULL,
            PRIMARY KEY (project_id, occurrence_key)
         );
         CREATE TABLE IF NOT EXISTS alert_outbox (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            transition TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            next_attempt_at_us INTEGER NOT NULL,
            sent_at_us INTEGER,
            last_error TEXT
         );
         CREATE INDEX IF NOT EXISTS alert_outbox_due
            ON alert_outbox (sent_at_us, next_attempt_at_us, id);
         CREATE INDEX IF NOT EXISTS alert_instances_project_status
            ON alert_instances (project_id, status, updated_at_us);",
    )
    .context("failed to create alert tables")
}

pub(crate) fn events_from_log_batch(
    project_id: &str,
    batch: &RecordBatch,
) -> Result<Vec<AlertEvent>> {
    let schema = batch.schema();
    let event_names = string_column(batch, schema.index_of("event_name")?)?;
    if !(0..batch.num_rows()).any(|row| {
        let name = event_names.value(row);
        name == OOM_FIRING_EVENT || name == OOM_RECOVERED_EVENT
    }) {
        return Ok(Vec::new());
    }
    let timestamps = batch
        .column(schema.index_of("timestamp")?)
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .context("timestamp column is not TimestampMicrosecondArray")?;
    let services = string_column(batch, schema.index_of("service_name")?)?;
    let resource_attrs = string_column(batch, schema.index_of("resource_attributes")?)?;
    let log_attrs = string_column(batch, schema.index_of("log_attributes")?)?;
    let mut events = Vec::new();

    for row in 0..batch.num_rows() {
        let event_name = event_names.value(row);
        if event_name != OOM_FIRING_EVENT && event_name != OOM_RECOVERED_EVENT {
            continue;
        }
        let resource = parse_attributes(resource_attrs, row)?;
        let attributes = parse_attributes(log_attrs, row)?;
        let service_name = resource
            .get("service.name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| services.value(row));
        anyhow::ensure!(
            service_name.len() <= 128,
            "service.name exceeds the alert label limit"
        );
        let container_name = required_string(&attributes, "container.name", 128)?;
        let container_id = required_string(&attributes, "container.id", 128)?;
        events.push(AlertEvent {
            project_id: project_id.to_string(),
            event_name: event_name.to_string(),
            timestamp_us: timestamps.value(row),
            service_name: service_name.to_string(),
            container_name: container_name.to_string(),
            container_id: container_id.to_string(),
        });
    }
    Ok(events)
}

fn process_firing(
    tx: &rusqlite::Transaction<'_>,
    event: &AlertEvent,
    fingerprint: &str,
    stored: Option<&StoredAlert>,
) -> Result<usize> {
    match stored {
        Some(stored)
            if stored.status == "resolved"
                && stored
                    .ends_at_us
                    .is_some_and(|ends_at| event.timestamp_us <= ends_at) =>
        {
            Ok(0)
        }
        Some(stored) if stored.status == "firing" => {
            tx.execute(
                "UPDATE alert_instances
                 SET occurrence_count = occurrence_count + 1, updated_at_us = ?3
                 WHERE project_id = ?1 AND fingerprint = ?2",
                params![event.project_id, fingerprint, event.timestamp_us],
            )?;
            Ok(0)
        }
        Some(stored) => {
            let count = stored.occurrence_count + 1;
            tx.execute(
                "UPDATE alert_instances SET status = 'firing', service_name = ?3,
                    container_name = ?4, starts_at_us = ?5, ends_at_us = NULL,
                    occurrence_count = ?6, updated_at_us = ?5
                 WHERE project_id = ?1 AND fingerprint = ?2",
                params![
                    event.project_id,
                    fingerprint,
                    event.service_name,
                    event.container_name,
                    event.timestamp_us,
                    count
                ],
            )?;
            enqueue(
                tx,
                &AlertTransition {
                    event,
                    fingerprint,
                    status: "firing",
                    starts_at_us: event.timestamp_us,
                    ends_at_us: None,
                    occurrence_count: count,
                },
            )?;
            Ok(1)
        }
        None => {
            tx.execute(
                "INSERT INTO alert_instances
                 (project_id, fingerprint, rule_id, status, service_name, container_name,
                  starts_at_us, ends_at_us, occurrence_count, updated_at_us)
                 VALUES (?1, ?2, ?3, 'firing', ?4, ?5, ?6, NULL, 1, ?6)",
                params![
                    event.project_id,
                    fingerprint,
                    OOM_RULE_ID,
                    event.service_name,
                    event.container_name,
                    event.timestamp_us
                ],
            )?;
            enqueue(
                tx,
                &AlertTransition {
                    event,
                    fingerprint,
                    status: "firing",
                    starts_at_us: event.timestamp_us,
                    ends_at_us: None,
                    occurrence_count: 1,
                },
            )?;
            Ok(1)
        }
    }
}

fn process_resolution(
    tx: &rusqlite::Transaction<'_>,
    event: &AlertEvent,
    fingerprint: &str,
    stored: &StoredAlert,
) -> Result<()> {
    tx.execute(
        "UPDATE alert_instances SET status = 'resolved', ends_at_us = ?3, updated_at_us = ?3
         WHERE project_id = ?1 AND fingerprint = ?2",
        params![event.project_id, fingerprint, event.timestamp_us],
    )?;
    let original = AlertEvent {
        project_id: event.project_id.clone(),
        event_name: event.event_name.clone(),
        timestamp_us: event.timestamp_us,
        service_name: stored.service_name.clone(),
        container_name: stored.container_name.clone(),
        container_id: event.container_id.clone(),
    };
    enqueue(
        tx,
        &AlertTransition {
            event: &original,
            fingerprint,
            status: "resolved",
            starts_at_us: stored.starts_at_us,
            ends_at_us: Some(event.timestamp_us),
            occurrence_count: stored.occurrence_count,
        },
    )
}

fn enqueue(tx: &rusqlite::Transaction<'_>, transition: &AlertTransition<'_>) -> Result<()> {
    let payload = webhook_payload(transition)?;
    tx.execute(
        "INSERT INTO alert_outbox
         (project_id, fingerprint, transition, payload_json, next_attempt_at_us)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            transition.event.project_id,
            transition.fingerprint,
            transition.status,
            payload,
            now_us()
        ],
    )?;
    Ok(())
}

fn webhook_payload(transition: &AlertTransition<'_>) -> Result<String> {
    let event = transition.event;
    let starts_at = format_timestamp(transition.starts_at_us)?;
    let ends_at = transition
        .ends_at_us
        .map(format_timestamp)
        .transpose()?
        .unwrap_or_else(|| "0001-01-01T00:00:00Z".to_string());
    serde_json::to_string(&serde_json::json!({
        "receiver": "sigiro",
        "status": transition.status,
        "alerts": [{
            "status": transition.status,
            "labels": {
                "alertname": "ContainerOOMKilled",
                "project": event.project_id,
                "service": event.service_name,
                "container": event.container_name,
                "severity": "critical",
            },
            "annotations": {
                "summary": format!("{} was killed because it ran out of memory", event.container_name),
                "occurrence_count": transition.occurrence_count.to_string(),
            },
            "startsAt": starts_at,
            "endsAt": ends_at,
            "generatorURL": "",
            "fingerprint": transition.fingerprint,
        }],
        "groupLabels": {"alertname": "ContainerOOMKilled"},
        "commonLabels": {
            "alertname": "ContainerOOMKilled",
            "project": event.project_id,
            "service": event.service_name,
            "container": event.container_name,
            "severity": "critical",
        },
        "commonAnnotations": {},
        "externalURL": "",
        "version": "4",
        "groupKey": format!("{{}}:{{alertname=\"ContainerOOMKilled\",fingerprint=\"{}\"}}", transition.fingerprint),
        "truncatedAlerts": 0,
    }))
    .context("failed to serialize Alertmanager webhook")
}

fn read_stored_alert(
    tx: &rusqlite::Transaction<'_>,
    project_id: &str,
    fingerprint: &str,
) -> Result<Option<StoredAlert>> {
    tx.query_row(
        "SELECT status, service_name, container_name, starts_at_us, ends_at_us, occurrence_count
         FROM alert_instances WHERE project_id = ?1 AND fingerprint = ?2",
        params![project_id, fingerprint],
        |row| {
            Ok(StoredAlert {
                status: row.get(0)?,
                service_name: row.get(1)?,
                container_name: row.get(2)?,
                starts_at_us: row.get(3)?,
                ends_at_us: row.get(4)?,
                occurrence_count: row.get(5)?,
            })
        },
    )
    .optional()
    .context("failed to read alert state")
}

fn alert_fingerprint(event: &AlertEvent) -> String {
    hash_hex(&[
        "sigiro/alert/v1",
        &event.project_id,
        OOM_RULE_ID,
        &event.service_name,
        &event.container_name,
    ])[..16]
        .to_string()
}

fn occurrence_key(event: &AlertEvent) -> String {
    hash_hex(&[
        "sigiro/alert-occurrence/v1",
        &event.project_id,
        &event.event_name,
        &event.timestamp_us.to_string(),
        &event.container_id,
    ])
}

fn hash_hex(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn format_timestamp(timestamp_us: i64) -> Result<String> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp_us) * 1_000)
        .context("alert timestamp is out of range")?
        .format(&Rfc3339)
        .context("failed to format alert timestamp")
}

fn now_us() -> i64 {
    (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000) as i64
}

fn string_column(batch: &RecordBatch, index: usize) -> Result<&StringArray> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .context("alert input column is not StringArray")
}

fn parse_attributes(
    column: &StringArray,
    row: usize,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    if column.is_null(row) {
        return Ok(serde_json::Map::new());
    }
    serde_json::from_str(column.value(row)).context("alert attributes are not a JSON object")
}

fn required_string<'a>(
    attributes: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    max_len: usize,
) -> Result<&'a str> {
    let value = attributes
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{key} is required for container OOM alerts"))?;
    anyhow::ensure!(
        value.len() <= max_len,
        "{key} exceeds the alert label limit"
    );
    Ok(value)
}
