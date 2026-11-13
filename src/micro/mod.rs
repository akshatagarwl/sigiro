//! Native MQTT ingress for the single Sigiro server binary.
//! The receipt boundary is FULL-synchronous raw inbox commit, not query visibility.
//! Broker ACLs authenticate topic ownership; hosted/JWT mode is deliberately refused.
mod config;
mod inbox;
#[cfg(test)]
mod tests;

use crate::inserter::MsgRouter;
use anyhow::{Context, Result, ensure};
use backon::{BackoffBuilder, ExponentialBuilder};
use config::Config;
use inbox::{Acceptance, Inbox};
use otlp_outbox::{Ack, BatchId, Outcome, Signal};
use prost::Message;
use rumqttc::v5::{
    AsyncClient, ConnectionError, Event, MqttOptions,
    mqttbytes::{
        QoS,
        v5::{Filter, Packet, Publish, PublishProperties, RetainForwardRule, SubscribeReasonCode},
    },
};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) struct Frame {
    device: String,
    project: String,
    id: BatchId,
    signal: Signal,
    payload: Vec<u8>,
}
pub(crate) struct Service {
    config: Config,
    inbox: Inbox,
    options: MqttOptions,
}
impl Service {
    pub(crate) async fn open(
        path: &Path,
        data_dir: &Path,
        auth: &crate::auth::AuthMode,
    ) -> Result<Arc<Self>> {
        ensure!(
            !auth.requires_auth(),
            "micro MQTT is currently limited to self-hosted open mode; hosted tenant authentication is not implemented"
        );
        let config = Config::load(path)?;
        let options = config.options()?;
        let inbox = Inbox::open(data_dir, config.max_inbox_bytes, config.max_batches).await?;
        Ok(Arc::new(Self {
            config,
            inbox,
            options,
        }))
    }
    fn frame(&self, packet: &Publish) -> Result<Frame> {
        ensure!(
            !packet.retain && packet.qos != QoS::AtMostOnce,
            "micro requires non-retained QoS 1/2 publications"
        );
        ensure!(
            packet.payload.len() <= self.config.max_batch_bytes as usize,
            "micro payload exceeds limit"
        );
        let topic = std::str::from_utf8(&packet.topic)?;
        let rest = topic
            .strip_prefix(&format!("{}/", self.config.namespace))
            .context("unexpected micro namespace")?;
        let (device, signal) = rest.split_once('/').context("missing micro signal")?;
        let binding = self
            .config
            .devices
            .iter()
            .find(|binding| binding.id == device)
            .context("unprovisioned micro device")?;
        let signal = match signal {
            "logs" => Signal::Logs,
            "traces" => Signal::Traces,
            "metrics" => Signal::Metrics,
            _ => anyhow::bail!("unsupported micro signal"),
        };
        let props = packet
            .properties
            .as_ref()
            .context("missing MQTT 5 properties")?;
        ensure!(
            props.content_type.as_deref() == Some("application/x-protobuf")
                && props.payload_format_indicator != Some(1),
            "micro requires binary OTLP protobuf"
        );
        let response = format!("{}/{device}/ack", self.config.namespace);
        ensure!(
            props.response_topic.as_deref() == Some(response.as_str()),
            "micro response topic is not provisioned for this device"
        );
        let id = props
            .correlation_data
            .as_ref()
            .context("missing stable batch ID")?;
        Ok(Frame {
            device: device.to_owned(),
            project: binding.project_id.clone(),
            id: BatchId(
                id.as_ref()
                    .try_into()
                    .context("batch ID must be 16 bytes")?,
            ),
            signal,
            payload: packet.payload.to_vec(),
        })
    }
    pub(crate) async fn mqtt(
        self: Arc<Self>,
        router: Arc<MsgRouter>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<()> {
        let (client, mut events) = AsyncClient::new(self.options.clone(), 64);
        let mut backoff = reconnect_backoff();
        loop {
            let event = tokio::select! { _ = stop.changed() => return Ok(()), event = events.poll() => event };
            match event {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    backoff = reconnect_backoff();
                    let topics = self.config.devices.iter().flat_map(|device| {
                        ["logs", "traces", "metrics"].map(|signal| {
                            let mut filter = Filter::new(
                                format!("{}/{}/{signal}", self.config.namespace, device.id),
                                QoS::AtLeastOnce,
                            );
                            // MQTT 5 otherwise clears RETAIN on live delivery,
                            // making a retained publisher appear non-retained.
                            filter.preserve_retain = true;
                            filter.retain_forward_rule = RetainForwardRule::Never;
                            filter
                        })
                    });
                    client
                        .try_subscribe_many(topics)
                        .context("queue micro MQTT subscription")?;
                }
                Ok(Event::Incoming(Packet::SubAck(ack))) => {
                    ensure!(
                        ack.return_codes
                            .iter()
                            .all(|code| matches!(code, SubscribeReasonCode::Success(_))),
                        "broker denied micro topic subscription"
                    );
                    tracing::info!(
                        devices = self.config.devices.len(),
                        "micro MQTT subscribed; receipts require durable raw storage"
                    );
                }
                Ok(Event::Incoming(Packet::Publish(packet))) => {
                    let frame = match self.frame(&packet) {
                        Ok(frame) => frame,
                        Err(error) => {
                            tracing::warn!(%error, "micro publication rejected before storage");
                            continue;
                        }
                    };
                    let id = frame.id;
                    let signal = frame.signal;
                    let response_topic = format!("{}/{}/ack", self.config.namespace, frame.device);
                    let outcome = if !valid_protobuf(frame.signal, &frame.payload) {
                        Outcome::Permanent
                    } else if router.disk_state().is_over_cap() {
                        Outcome::RetryLater
                    } else {
                        match self.inbox.accept(frame).await? {
                            Acceptance::Stored => Outcome::FullAcceptance,
                            Acceptance::Quota => Outcome::RetryLater,
                            Acceptance::Conflict => Outcome::Permanent,
                        }
                    };
                    let receipt = Ack {
                        id,
                        signal,
                        outcome,
                    }
                    .encode();
                    let properties = PublishProperties {
                        correlation_data: Some(id.0.to_vec().into()),
                        content_type: Some("application/vnd.sigiro.micro-receipt.v1".into()),
                        ..Default::default()
                    };
                    // Never await this bounded client queue while owning eventloop
                    // polling: saturation would deadlock the reactor. A dropped ACK
                    // is safe: retry finds the same committed raw inbox row.
                    if let Err(error) = client.try_publish_with_properties(
                        response_topic,
                        QoS::AtLeastOnce,
                        false,
                        receipt.to_vec(),
                        properties,
                    ) {
                        tracing::warn!(%error, "micro receipt queue full; device must retry");
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    if fatal_transport_error(&error) {
                        return Err(error.into());
                    }
                    tracing::warn!(%error, "micro broker disconnected; reconnecting with backoff");
                    let delay = backoff
                        .next()
                        .context("micro reconnect retries exhausted")?;
                    tokio::select! { _ = stop.changed() => return Ok(()), _ = tokio::time::sleep(delay) => {} }
                }
            }
        }
    }
    pub(crate) async fn project(
        self: Arc<Self>,
        router: Arc<MsgRouter>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<()> {
        loop {
            if *stop.borrow() {
                return Ok(());
            }
            let pending = self.inbox.next().await?;
            if let Some(pending) = pending {
                let result = crate::ingest::project_micro(
                    &router,
                    pending.frame.project,
                    pending.frame.signal,
                    pending.frame.payload,
                )
                .await;
                match result {
                    Ok(()) => self.inbox.finish(pending.row, None).await?,
                    Err(error) => {
                        let diagnostic: String = error.message().chars().take(512).collect();
                        if error.code() == tonic::Code::InvalidArgument {
                            self.inbox.finish(pending.row, Some(diagnostic)).await?
                        } else {
                            self.inbox.retry(pending.row, diagnostic).await?
                        }
                    }
                }
            } else {
                tokio::select! { _ = stop.changed() => return Ok(()), _ = tokio::time::sleep(Duration::from_millis(100)) => {} }
            }
        }
    }
}
fn reconnect_backoff() -> backon::ExponentialBackoff {
    ExponentialBuilder::default()
        .with_min_delay(Duration::from_secs(1))
        // backon adds up to one base delay of jitter: total stays <=30s.
        .with_max_delay(Duration::from_secs(15))
        .with_jitter()
        .without_max_times()
        .build()
}

fn fatal_transport_error(error: &ConnectionError) -> bool {
    use rumqttc::v5::{TlsError, mqttbytes::v5::ConnectReturnCode};
    match error {
        ConnectionError::ConnectionRefused(code) => !matches!(
            code,
            ConnectReturnCode::ServiceUnavailable
                | ConnectReturnCode::ServerUnavailable
                | ConnectReturnCode::ServerBusy
                | ConnectReturnCode::QuotaExceeded
                | ConnectReturnCode::ConnectionRateExceeded
                | ConnectReturnCode::UnspecifiedError
        ),
        // Reset/EOF during TLS negotiation is transient, not a trust failure.
        ConnectionError::Tls(TlsError::Io(error)) => {
            matches!(
                error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
            ) || error
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<rustls::Error>)
        }
        ConnectionError::Tls(_) => true,
        _ => false,
    }
}

/// Read local raw-inbox state without starting a broker connection or creating
/// storage. Counts describe recording/projection, not device connectivity.
pub fn status(data_dir: &Path) -> Result<serde_json::Value> {
    use rusqlite::{Connection, OpenFlags};
    let mut connection = Connection::open_with_flags(
        data_dir.join(".micro/inbox.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let transaction = connection.transaction()?;
    let version: u32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(version == 1, "unsupported micro inbox schema");
    let mut statement = transaction.prepare("SELECT device,state,COUNT(*),SUM(length(payload)),SUM(attempts>0) FROM batches GROUP BY device,state ORDER BY device,state")?;
    let devices = statement.query_map([], |row| Ok(serde_json::json!({"device":row.get::<_,String>(0)?, "state":row.get::<_,String>(1)?, "batches":row.get::<_,u64>(2)?, "payload_bytes":row.get::<_,u64>(3)?, "previously_retried":row.get::<_,u64>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let (batches, bytes): (u64, u64) = transaction.query_row(
        "SELECT batches,bytes FROM usage WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(
        serde_json::json!({"schema_version":version,"receipt_boundary":"durably_recorded", "projection_delivery":"at_least_once", "batches":batches,"payload_bytes":bytes,"devices":devices}),
    )
}

fn valid_protobuf(signal: Signal, bytes: &[u8]) -> bool {
    use opentelemetry_proto::tonic::collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    };
    match signal {
        Signal::Logs => ExportLogsServiceRequest::decode(bytes).is_ok(),
        Signal::Traces => ExportTraceServiceRequest::decode(bytes).is_ok(),
        Signal::Metrics => ExportMetricsServiceRequest::decode(bytes).is_ok(),
    }
}
