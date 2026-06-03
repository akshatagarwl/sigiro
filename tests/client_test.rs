use sigiro::client::{ClientAuth, SigiroClient};

#[test]
fn build_client_with_url() {
    let client = SigiroClient::new(
        "https://app.sigiro.com/",
        ClientAuth::Bearer {
            token: "sk_live_test".to_string(),
        },
    );
    assert_eq!(client.base_url(), "https://app.sigiro.com");
}

#[test]
fn build_client_trims_trailing_slashes() {
    let client = SigiroClient::new(
        "https://example.com///",
        ClientAuth::Bearer {
            token: "key".to_string(),
        },
    );
    assert_eq!(client.base_url(), "https://example.com");
}

#[test]
fn build_client_default_endpoint() {
    let client = SigiroClient::new(
        "https://app.sigiro.com",
        ClientAuth::Bearer {
            token: "sk_live_test".to_string(),
        },
    );
    assert_eq!(client.base_url(), "https://app.sigiro.com");
}

#[test]
fn build_client_open_mode() {
    let client = SigiroClient::new("http://localhost:9999", ClientAuth::Open);
    assert_eq!(client.base_url(), "http://localhost:9999");
}
