use sigiro::client::SigiroClient;

#[test]
fn build_client_with_url() {
    let client = SigiroClient::new("https://app.sigiro.com/", "sk_live_test");
    assert_eq!(client.base_url(), "https://app.sigiro.com");
}

#[test]
fn build_client_trims_trailing_slashes() {
    let client = SigiroClient::new("https://example.com///", "key");
    assert_eq!(client.base_url(), "https://example.com");
}

#[test]
fn build_client_default_endpoint() {
    let client = SigiroClient::new("https://app.sigiro.com", "sk_live_test");
    assert_eq!(client.base_url(), "https://app.sigiro.com");
}
