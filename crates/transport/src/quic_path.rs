//! QUIC path: sends keyframes (and promoted P-frames) reliably via quinn.
//!
//! ## Why QUIC for keyframes?
//!
//! A keyframe is the reference point for all subsequent P-frames in a GOP.
//! If a keyframe is lost and not retransmitted, the decoder must wait for the
//! next keyframe — typically 2–4 seconds at 0.5fps keyframe rate. QUIC's
//! reliable stream delivery prevents this at the cost of ~1 RTT on loss events.
//!
//! P-frames are routed here only when [`AdaptationDecision::UseQuic`] is active
//! (i.e., SRT loss > 0.5%).

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use quinn::{Connection, Endpoint};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::metrics::TransportMetrics;

/// Owns the QUIC endpoint and connection.
///
/// Receives serialised frame payloads from the [`FrameRouter`] and sends them
/// on a QUIC unidirectional stream per frame (avoids stream-level HOL blocking
/// between independent frames).
pub struct QuicPath {
    endpoint: Endpoint,
    remote_addr: SocketAddr,
    metrics: Arc<TransportMetrics>,
}

impl QuicPath {
    /// Creates a QUIC endpoint bound to `local_addr`.
    ///
    /// Call [`QuicPath::run`] to connect and start the send loop.
    pub async fn new(
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
        server_config: quinn::ServerConfig,
        metrics: Arc<TransportMetrics>,
    ) -> anyhow::Result<Self> {
        let endpoint = Endpoint::server(server_config, local_addr)?;
        Ok(Self {
            endpoint,
            remote_addr,
            metrics,
        })
    }

    /// Connects to the remote and starts forwarding payloads from `rx`.
    ///
    /// Each frame payload is sent on a fresh unidirectional QUIC stream.
    /// Opening a new stream per frame ensures that a lost packet for frame N
    /// does not delay delivery of frame N+1.
    pub async fn run_sender(
        self,
        client_config: quinn::ClientConfig,
        mut rx: mpsc::Receiver<Bytes>,
    ) -> anyhow::Result<()> {
        // Connect as a QUIC client to the relay/receiver
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
        endpoint.set_default_client_config(client_config);

        info!("QUIC: connecting to {}", self.remote_addr);
        let conn: Connection = endpoint
            .connect(self.remote_addr, "aether.local")?
            .await?;
        info!("QUIC: connected to {}", self.remote_addr);

        while let Some(payload) = rx.recv().await {
            let conn = conn.clone();
            let metrics = Arc::clone(&self.metrics);

            // Each frame on its own stream — no inter-frame HOL blocking
            tokio::spawn(async move {
                match conn.open_uni().await {
                    Ok(mut stream) => {
                        if let Err(e) = stream.write_all(&payload).await {
                            warn!("QUIC stream write error: {}", e);
                            metrics
                                .packets_lost
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        } else {
                            let _ = stream.finish();
                            metrics
                                .packets_sent
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            metrics
                                .keyframes_quic
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            debug!("QUIC: sent {} bytes", payload.len());
                        }
                    }
                    Err(e) => {
                        warn!("QUIC open_uni failed: {}", e);
                    }
                }
            });
        }
        Ok(())
    }

    /// Accepts incoming QUIC streams and forwards reassembled payloads to `tx`.
    pub async fn run_receiver(
        self,
        tx: mpsc::Sender<Bytes>,
    ) -> anyhow::Result<()> {
        info!("QUIC: listening on {}", self.endpoint.local_addr()?);
        while let Some(incoming) = self.endpoint.accept().await {
            let conn = incoming.await?;
            let tx = tx.clone();
            tokio::spawn(async move {
                loop {
                    match conn.accept_uni().await {
                        Ok(mut stream) => {
                            match stream.read_to_end(1024 * 1024).await {
                                Ok(data) => {
                                    let _ = tx.send(Bytes::from(data)).await;
                                }
                                Err(e) => warn!("QUIC read error: {}", e),
                            }
                        }
                        Err(e) => {
                            debug!("QUIC connection closed: {}", e);
                            break;
                        }
                    }
                }
            });
        }
        Ok(())
    }
}
