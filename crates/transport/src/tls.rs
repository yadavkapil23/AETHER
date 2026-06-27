//! TLS configuration for the QUIC (quinn) connection.
//!
//! Generates a self-signed certificate at startup — no cert files needed.
//! In production, swap `generate_self_signed` for a proper PKI.

use anyhow::Result;
use quinn::{ClientConfig, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use std::sync::Arc;

/// Generates an ephemeral self-signed TLS certificate and returns both
/// the server and client configurations that trust it.
pub fn generate_self_signed_configs() -> Result<(ServerConfig, ClientConfig)> {
    // Use rcgen to generate a self-signed cert
    let cert = rcgen::generate_simple_self_signed(vec!["aether.local".to_string()])
        .map_err(|e| anyhow::anyhow!("cert generation failed: {}", e))?;

    let cert_der = CertificateDer::from(cert.cert);
    let key_der = PrivateKeyDer::try_from(cert.key_pair.serialize_der())
        .map_err(|e| anyhow::anyhow!("key serialization failed: {e}"))?;

    let server_config = ServerConfig::with_single_cert(vec![cert_der.clone()], key_der)?;

    let client_config = {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert_der)?;
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(config)
            .map_err(|e| anyhow::anyhow!("invalid client config: {}", e))?;
        ClientConfig::new(Arc::new(quic_config))
    };

    Ok((server_config, client_config))
}
