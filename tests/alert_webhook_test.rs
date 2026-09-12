use sigiro::alerts::AlertWebhookToken;

#[test]
fn alert_webhook_token_rejects_empty_credentials() {
    assert!(AlertWebhookToken::try_new(String::new()).is_err());

    let token = AlertWebhookToken::try_new("receiver-secret".to_string()).unwrap();
    assert_eq!(token.as_ref(), "receiver-secret");
}
