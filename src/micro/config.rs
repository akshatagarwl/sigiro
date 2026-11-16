use anyhow::{Context, Result, ensure};
use rumqttc::{Transport, v5::MqttOptions};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub broker: Broker,
    pub namespace: String,
    pub devices: Vec<Device>,
    pub max_batch_bytes: u32,
    pub max_inbox_bytes: u64,
    pub max_batches: u64,
    #[serde(default)]
    pub allow_insecure_loopback: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Broker {
    pub host: String,
    pub port: u16,
    pub client_id: String,
    pub tls: bool,
    pub username: Option<String>,
    pub password_file: Option<PathBuf>,
    pub ca_file: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub id: String,
    pub project_id: String,
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(65537)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 65536, "micro configuration exceeds 64 KiB");
        let mut config: Self =
            serde_json::from_slice(&bytes).context("invalid micro configuration")?;
        // Credential/CA paths are relative to the configuration, not server cwd.
        let parent = path.parent().unwrap_or(Path::new("."));
        for file in [&mut config.broker.password_file, &mut config.broker.ca_file]
            .into_iter()
            .flatten()
        {
            if file.is_relative() {
                *file = parent.join(&*file);
            }
        }
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.broker.host.is_empty() && self.broker.host.len() <= 253 && self.broker.port != 0,
            "invalid MQTT host/port"
        );
        ensure!(identifier(&self.broker.client_id), "invalid MQTT client ID");
        ensure!(
            self.namespace.len() <= 192 && self.namespace.split('/').all(identifier),
            "invalid MQTT namespace"
        );
        ensure!(
            !self.devices.is_empty() && self.devices.len() <= 32,
            "configure between 1 and 32 devices"
        );
        let mut ids = BTreeSet::new();
        for device in &self.devices {
            ensure!(
                identifier(&device.id)
                    && crate::storage::sanitize_tenant_id(&device.project_id).is_ok()
                    && ids.insert(&device.id),
                "invalid/duplicate device binding"
            );
        }
        ensure!(
            (1..=1_048_576).contains(&self.max_batch_bytes),
            "max_batch_bytes must be 1..=1048576"
        );
        ensure!(
            self.max_inbox_bytes >= u64::from(self.max_batch_bytes)
                && self.max_inbox_bytes <= i64::MAX as u64,
            "invalid inbox byte quota"
        );
        ensure!(
            (1..=1_000_000).contains(&self.max_batches),
            "invalid inbox batch quota"
        );
        // Plaintext must use a literal loopback IP, not DNS that might resolve
        // elsewhere (even a hostname spelled localhost).
        let loopback = self
            .broker
            .host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
        ensure!(
            self.broker.tls || (self.allow_insecure_loopback && loopback),
            "MQTT requires verified TLS; plaintext is allowed only for explicitly enabled loopback tests"
        );
        ensure!(
            self.broker.username.is_some() == self.broker.password_file.is_some(),
            "MQTT username and password_file must be configured together"
        );
        ensure!(
            self.broker
                .username
                .as_ref()
                .is_none_or(|user| !user.is_empty() && user.len() <= 128),
            "invalid MQTT username"
        );
        ensure!(
            self.broker.username.is_some() || (self.allow_insecure_loopback && loopback),
            "MQTT requires broker credentials outside explicit loopback tests"
        );
        ensure!(
            self.broker.tls || self.broker.ca_file.is_none(),
            "ca_file requires TLS"
        );
        Ok(())
    }
    pub fn options(&self) -> Result<MqttOptions> {
        let mut options =
            MqttOptions::new(&self.broker.client_id, &self.broker.host, self.broker.port);
        options.set_keep_alive(Duration::from_secs(30));
        options.set_connection_timeout(10);
        options.set_max_packet_size(Some(self.max_batch_bytes + 4096));
        options.set_receive_maximum(Some(8));
        options.set_outgoing_inflight_upper_limit(16);
        if self.broker.tls {
            use rustls::pki_types::{CertificateDer, pem::PemObject};
            let certificates = match &self.broker.ca_file {
                Some(path) => CertificateDer::pem_slice_iter(
                    &std::fs::read(path).context("read MQTT CA certificate")?,
                )
                .collect::<std::result::Result<Vec<_>, _>>()?,
                None => rustls_native_certs::load_native_certs().certs,
            };
            ensure!(!certificates.is_empty(), "no MQTT trust anchors available");
            let mut roots = rustls::RootCertStore::empty();
            for certificate in certificates {
                roots
                    .add(certificate)
                    .context("invalid MQTT trust anchor")?;
            }
            // Dependencies enable both ring and aws-lc; never rely on rustls's
            // ambiguous global provider auto-selection (which panics).
            let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
            options.set_transport(Transport::tls_with_config(tls.into()));
        }
        if let (Some(user), Some(path)) = (&self.broker.username, &self.broker.password_file) {
            use std::io::Read;
            let file = std::fs::File::open(path).context("open MQTT password file")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                ensure!(
                    file.metadata()?.permissions().mode() & 0o077 == 0,
                    "MQTT password file must not be group/world accessible"
                );
            }
            let mut password = String::new();
            file.take(4097).read_to_string(&mut password)?;
            ensure!(
                !password.trim_end_matches(['\r', '\n']).is_empty() && password.len() <= 4096,
                "invalid MQTT password length"
            );
            options.set_credentials(user, password.trim_end_matches(['\r', '\n']));
        }
        Ok(options)
    }
}
