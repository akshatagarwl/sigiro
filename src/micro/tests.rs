use super::*;
use tempfile::TempDir;

fn frame(id: u8) -> Frame {
    Frame {
        device: "a".into(),
        project: "default".into(),
        id: BatchId([id; 16]),
        signal: Signal::Logs,
        payload: b"\xa0\x06\x01".to_vec(),
    }
}
fn config() -> Config {
    serde_json::from_value(serde_json::json!({"broker":{"host":"127.0.0.1","port":1883,"client_id":"server","tls":false},"namespace":"sigiro/micro/v1","devices":[{"id":"a","project_id":"default"}],"max_batch_bytes":64,"max_inbox_bytes":128,"max_batches":2,"allow_insecure_loopback":true})).unwrap()
}
#[tokio::test]
async fn receipt_survives_reopen_with_exact_unknown_bytes() {
    let temp = TempDir::new().unwrap();
    let original = frame(1);
    {
        let inbox = Inbox::open(temp.path(), 128, 2).await.unwrap();
        assert_eq!(
            inbox.accept(original.clone()).await.unwrap(),
            Acceptance::Stored
        );
    }
    let inbox = Inbox::open(temp.path(), 128, 2).await.unwrap();
    assert_eq!(
        inbox.accept(original.clone()).await.unwrap(),
        Acceptance::Stored
    );
    let pending = inbox.next().await.unwrap().unwrap();
    assert_eq!(pending.frame.payload, original.payload);
    inbox.finish(pending.row, None).await.unwrap();
    assert!(inbox.next().await.unwrap().is_none());
    assert_eq!(inbox.accept(original).await.unwrap(), Acceptance::Stored);
}
#[tokio::test]
async fn same_id_is_not_a_new_batch_or_a_cross_signal_receipt() {
    let temp = TempDir::new().unwrap();
    let inbox = Inbox::open(temp.path(), 128, 2).await.unwrap();
    let mut value = frame(1);
    inbox.accept(value.clone()).await.unwrap();
    value.payload.push(0);
    assert_eq!(inbox.accept(value).await.unwrap(), Acceptance::Conflict);
    value = frame(1);
    value.signal = Signal::Traces;
    assert_eq!(inbox.accept(value).await.unwrap(), Acceptance::Conflict);
    value = frame(1);
    value.project = "other".into();
    assert_eq!(inbox.accept(value).await.unwrap(), Acceptance::Conflict);
    value = frame(1);
    value.device = "b".into();
    assert_eq!(inbox.accept(value).await.unwrap(), Acceptance::Stored);
}
#[tokio::test]
async fn quota_never_overwrites_and_duplicates_work_when_full() {
    let temp = TempDir::new().unwrap();
    let inbox = Inbox::open(temp.path(), 3, 1).await.unwrap();
    assert_eq!(inbox.accept(frame(1)).await.unwrap(), Acceptance::Stored);
    assert_eq!(inbox.accept(frame(2)).await.unwrap(), Acceptance::Quota);
    assert_eq!(inbox.accept(frame(1)).await.unwrap(), Acceptance::Stored);
    let pending = inbox.next().await.unwrap().unwrap();
    inbox
        .finish(pending.row, Some("unsupported projection".into()))
        .await
        .unwrap();
    assert!(inbox.next().await.unwrap().is_none());
    assert_eq!(inbox.accept(frame(2)).await.unwrap(), Acceptance::Quota); // quarantine is not deletion
}
#[tokio::test]
async fn byte_quota_is_independent_of_batch_count() {
    let temp = TempDir::new().unwrap();
    let inbox = Inbox::open(temp.path(), 5, 10).await.unwrap();
    assert_eq!(inbox.accept(frame(1)).await.unwrap(), Acceptance::Stored);
    assert_eq!(inbox.accept(frame(2)).await.unwrap(), Acceptance::Quota);
}
#[tokio::test]
async fn retry_does_not_block_other_devices() {
    let temp = TempDir::new().unwrap();
    let inbox = Inbox::open(temp.path(), 128, 10).await.unwrap();
    inbox.accept(frame(1)).await.unwrap();
    inbox.accept(frame(2)).await.unwrap();
    let first = inbox.next().await.unwrap().unwrap();
    inbox.retry(first.row, "temporary".into()).await.unwrap();
    assert_eq!(
        inbox.next().await.unwrap().unwrap().frame.id,
        BatchId([2; 16])
    );
}
#[tokio::test]
async fn concurrent_duplicates_commit_one_row() {
    let temp = TempDir::new().unwrap();
    let inbox = Inbox::open(temp.path(), 128, 1).await.unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let inbox = inbox.clone();
        tasks.spawn(async move { inbox.accept(frame(1)).await });
    }
    while let Some(result) = tasks.join_next().await {
        assert_eq!(result.unwrap().unwrap(), Acceptance::Stored);
    }
    assert_eq!(inbox.accept(frame(2)).await.unwrap(), Acceptance::Quota);
    assert_eq!(status(temp.path()).unwrap()["batches"], 1);
}
#[tokio::test]
async fn checksum_corruption_fails_closed_on_replay_and_redelivery() {
    let temp = TempDir::new().unwrap();
    let inbox = Inbox::open(temp.path(), 128, 2).await.unwrap();
    inbox.accept(frame(1)).await.unwrap();
    let connection = rusqlite::Connection::open(temp.path().join(".micro/inbox.sqlite")).unwrap();
    connection
        .execute("UPDATE batches SET payload=x'a00602'", [])
        .unwrap();
    assert!(inbox.next().await.is_err());
    assert!(inbox.accept(frame(1)).await.is_err());
}
#[tokio::test]
async fn receipt_row_and_quota_counter_rollback_together() {
    let temp = TempDir::new().unwrap();
    let inbox = Inbox::open(temp.path(), 128, 2).await.unwrap();
    let connection = rusqlite::Connection::open(temp.path().join(".micro/inbox.sqlite")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_counter BEFORE UPDATE ON usage BEGIN SELECT RAISE(ABORT,'injected counter failure'); END;").unwrap();
    assert!(inbox.accept(frame(1)).await.is_err());
    assert!(inbox.next().await.unwrap().is_none());
    assert_eq!(status(temp.path()).unwrap()["batches"], 0);
    connection
        .execute_batch("DROP TRIGGER fail_counter")
        .unwrap();
    assert_eq!(inbox.accept(frame(1)).await.unwrap(), Acceptance::Stored);
}
#[tokio::test]
async fn status_is_read_only_and_micro_directory_is_not_a_project() {
    let temp = TempDir::new().unwrap();
    assert!(status(temp.path()).is_err());
    assert!(!temp.path().join(".micro").exists());
    let inbox = Inbox::open(temp.path(), 128, 2).await.unwrap();
    inbox.accept(frame(1)).await.unwrap();
    assert_eq!(status(temp.path()).unwrap()["batches"], 1);
    assert!(crate::storage::sanitize_tenant_id(".micro").is_err());
}
#[test]
fn reconnect_backoff_is_unbounded_in_attempts_but_bounded_in_delay() {
    let delays: Vec<_> = reconnect_backoff().take(1024).collect();
    assert_eq!(delays.len(), 1024);
    assert!(
        delays
            .iter()
            .all(|delay| (Duration::from_secs(1)..Duration::from_secs(30)).contains(delay))
    );
    assert!(reconnect_backoff().next().unwrap() < Duration::from_secs(2));
}
#[test]
fn network_outage_is_retryable_but_trust_and_auth_failures_are_not() {
    use rumqttc::v5::{TlsError, mqttbytes::v5::ConnectReturnCode};
    assert!(!fatal_transport_error(&ConnectionError::Tls(TlsError::Io(
        std::io::ErrorKind::UnexpectedEof.into()
    ))));
    assert!(!fatal_transport_error(&ConnectionError::ConnectionRefused(
        ConnectReturnCode::ServerBusy
    )));
    assert!(fatal_transport_error(&ConnectionError::ConnectionRefused(
        ConnectReturnCode::NotAuthorized
    )));
    assert!(fatal_transport_error(&ConnectionError::Tls(TlsError::Io(
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer)
        )
    ))));
}
#[test]
fn strict_admission_keeps_unknown_fields_but_rejects_truncation() {
    for signal in [Signal::Logs, Signal::Traces, Signal::Metrics] {
        assert!(valid_protobuf(signal, b"\xa0\x06\x01"));
        assert!(!valid_protobuf(signal, b"\x0a\xff\xff"));
    }
}
#[test]
fn configuration_rejects_unsafe_routes_and_plaintext_remote() {
    config().validate().unwrap();
    let mut value = config();
    value.broker.host = "broker.example".into();
    assert!(value.validate().is_err());
    value = config();
    value.devices[0].id = "+".into();
    assert!(value.validate().is_err());
    value = config();
    value.devices[0].project_id = "../other".into();
    assert!(value.validate().is_err());
    value = config();
    value.devices[0].project_id = "extensions".into();
    assert!(value.validate().is_err());
    value = config();
    value.devices[0].project_id = "invalid-hyphen".into();
    assert!(value.validate().is_err());
    value = config();
    value.namespace = "sigiro/#".into();
    assert!(value.validate().is_err());
    value = config();
    value.allow_insecure_loopback = false;
    assert!(value.validate().is_err());
    value = config();
    value.broker.username = Some("user".into());
    assert!(value.validate().is_err());
}
#[tokio::test]
async fn frame_does_not_trust_sender_response_topic_or_project() {
    let temp = TempDir::new().unwrap();
    let config = config();
    let inbox = Inbox::open(temp.path(), 128, 2).await.unwrap();
    let service = Service {
        options: config.options().unwrap(),
        config,
        inbox,
    };
    let mut packet = Publish::new(
        "sigiro/micro/v1/a/logs",
        QoS::AtLeastOnce,
        frame(1).payload,
        Some(PublishProperties {
            response_topic: Some("sigiro/micro/v1/a/ack".into()),
            correlation_data: Some(vec![1; 16].into()),
            content_type: Some("application/x-protobuf".into()),
            ..Default::default()
        }),
    );
    assert_eq!(service.frame(&packet).unwrap().project, "default");
    packet.properties.as_mut().unwrap().response_topic = Some("sigiro/micro/v1/b/ack".into());
    assert!(service.frame(&packet).is_err());
    packet.properties.as_mut().unwrap().response_topic = Some("sigiro/micro/v1/a/ack".into());
    packet.retain = true;
    assert!(service.frame(&packet).is_err());
    packet.retain = false;
    packet.properties.as_mut().unwrap().correlation_data = Some(vec![1; 15].into());
    assert!(service.frame(&packet).is_err());
}
