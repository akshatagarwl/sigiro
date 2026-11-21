//! HTTP clients that work on any machine.
//!
//! reqwest verifies servers against the operating system's trust store, and
//! refuses to even build a client when that store is empty: a minimal
//! container, a scratch image, a bare chroot. sigiro is one static binary
//! meant to run anywhere, so every client also trusts Mozilla's root set,
//! compiled in (`webpki-root-certs`). The system store still applies wherever
//! it exists, including any private CAs it adds.
//!
//! Build every reqwest client through [`http_client`] or
//! [`blocking_http_client`]; clippy.toml rejects the plain constructors.

use std::sync::LazyLock;

static BUNDLED_ROOTS: LazyLock<Vec<reqwest::Certificate>> = LazyLock::new(|| {
    webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .filter_map(|der| reqwest::Certificate::from_der(der).ok())
        .collect()
});

/// A `reqwest::ClientBuilder` trusting the system store plus Mozilla's roots.
#[allow(clippy::disallowed_methods)]
pub fn http_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder().tls_certs_merge(BUNDLED_ROOTS.iter().cloned())
}

/// The blocking counterpart of [`http_client`].
#[allow(clippy::disallowed_methods)]
pub fn blocking_http_client() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder().tls_certs_merge(BUNDLED_ROOTS.iter().cloned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn mozilla_roots_are_bundled() {
        assert!(super::BUNDLED_ROOTS.len() > 100);
    }

    #[test]
    fn clients_build() {
        super::http_client().build().expect("async client");
        super::blocking_http_client()
            .build()
            .expect("blocking client");
    }
}
