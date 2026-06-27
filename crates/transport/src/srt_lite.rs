//! SRT-lite: lightweight UDP transport with sequence numbers and selective ARQ.
//!
//! ## Protocol
//!
//! ```text
//! Header (8 bytes):
//!   [0..4]  sequence number (u32, big-endian)
//!   [4]     flags: bit 0 = is_retransmit, bit 1 = is_ack, bit 2 = is_nak
//!   [5]     payload_type (maps to RTP payload_type)
//!   [6..8]  payload length (u16, big-endian)
//!
//! ACK packet (flags = 0x02):
//!   [8..12] ack_up_to (u32) — all seqs up to this are received
//!   [12..]  selective NAK list: u32 sequence numbers of missing packets
//!
//! Data packet (flags = 0x00 or 0x01):
//!   [8..]   payload bytes
//! ```
//!
//! The sender maintains a retransmit buffer. On receiving a NAK, it immediately
//! resends the missing packet. There is no FEC — keyframes are handled by QUIC.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tokio::time::interval;
use tracing::{debug, trace, warn};

use crate::metrics::TransportMetrics;

// ──────────────────────────────────────────────────────────────────────────────
// Wire format constants
// ──────────────────────────────────────────────────────────────────────────────

const HEADER_SIZE: usize = 8;
const FLAG_RETRANSMIT: u8 = 0x01;
const FLAG_ACK: u8 = 0x02;
const FLAG_NAK: u8 = 0x04;
const MAX_UDP_PAYLOAD: usize = 1400; // conservative MTU safety margin

// ──────────────────────────────────────────────────────────────────────────────
// Packet encoding / decoding
// ──────────────────────────────────────────────────────────────────────────────

fn encode_data_packet(seq: u32, payload_type: u8, payload: &[u8], is_retransmit: bool) -> Bytes {
    let flags = if is_retransmit { FLAG_RETRANSMIT } else { 0 };
    let len = payload.len() as u16;
    let mut buf = BytesMut::with_capacity(HEADER_SIZE + payload.len());
    buf.extend_from_slice(&seq.to_be_bytes());
    buf.extend_from_slice(&[flags, payload_type]);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    buf.freeze()
}

fn encode_ack_packet(ack_up_to: u32, naks: &[u32]) -> Bytes {
    let mut buf = BytesMut::with_capacity(HEADER_SIZE + 4 + naks.len() * 4);
    buf.extend_from_slice(&0u32.to_be_bytes()); // seq=0 (unused in ack)
    buf.extend_from_slice(&[FLAG_ACK, 0u8]);
    buf.extend_from_slice(&0u16.to_be_bytes()); // len=0
    buf.extend_from_slice(&ack_up_to.to_be_bytes());
    for &n in naks {
        buf.extend_from_slice(&n.to_be_bytes());
    }
    buf.freeze()
}

// ──────────────────────────────────────────────────────────────────────────────
// RetransmitBuffer
// ──────────────────────────────────────────────────────────────────────────────

/// In-memory buffer of sent packets awaiting ACK, for retransmission on NAK.
struct RetransmitBuffer {
    /// seq → (encoded packet, sent_at)
    packets: HashMap<u32, (Bytes, Instant)>,
    /// Evict entries older than this
    max_age: Duration,
}

impl RetransmitBuffer {
    fn new() -> Self {
        Self {
            packets: HashMap::new(),
            max_age: Duration::from_millis(500),
        }
    }

    fn insert(&mut self, seq: u32, pkt: Bytes) {
        self.packets.insert(seq, (pkt, Instant::now()));
    }

    fn get(&self, seq: u32) -> Option<&Bytes> {
        self.packets.get(&seq).map(|(pkt, _)| pkt)
    }

    fn acknowledge_up_to(&mut self, ack_up_to: u32) {
        self.packets.retain(|&seq, _| seq > ack_up_to);
    }

    /// Removes packets older than `max_age` to bound memory usage.
    fn evict_stale(&mut self) {
        let cutoff = Instant::now() - self.max_age;
        self.packets.retain(|_, (_, sent_at)| *sent_at > cutoff);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// SrtSender
// ──────────────────────────────────────────────────────────────────────────────

/// Sends P-frame payloads over UDP with sequence numbers and retransmit support.
///
/// Receives raw payloads from the [`FrameRouter`] via an mpsc channel.
/// Reads NAK lists from the receiver via the same socket.
pub struct SrtSender {
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    metrics: Arc<TransportMetrics>,
}

impl SrtSender {
    pub fn new(
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
        metrics: Arc<TransportMetrics>,
    ) -> Self {
        Self {
            local_addr,
            remote_addr,
            metrics,
        }
    }

    /// Binds the UDP socket and starts the send/retransmit loop.
    ///
    /// `rx`: channel from the router delivering payloads to send.
    pub async fn run(self, mut rx: mpsc::Receiver<Bytes>) -> anyhow::Result<()> {
        let sock = Arc::new(UdpSocket::bind(self.local_addr).await?);
        sock.connect(self.remote_addr).await?;

        let retransmit_buf = Arc::new(Mutex::new(RetransmitBuffer::new()));
        let mut seq: u32 = 0;
        let mut evict_tick = interval(Duration::from_millis(100));

        // Spawn a reader task that handles incoming ACK/NAK packets
        let sock_reader = Arc::clone(&sock);
        let buf_reader = Arc::clone(&retransmit_buf);
        let metrics_reader = Arc::clone(&self.metrics);

        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                match sock_reader.recv(&mut buf).await {
                    Ok(n) if n >= HEADER_SIZE => {
                        let flags = buf[4];
                        if flags & FLAG_ACK != 0 {
                            let ack_up_to = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
                            let mut rb = buf_reader.lock().await;
                            rb.acknowledge_up_to(ack_up_to);

                            // NAKs follow the ack_up_to field
                            let nak_bytes = &buf[12..n];
                            let naks: Vec<u32> = nak_bytes
                                .chunks_exact(4)
                                .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                                .collect();

                            for nak_seq in naks {
                                if let Some(pkt) = rb.get(nak_seq) {
                                    let pkt = pkt.clone();
                                    let _ = sock_reader.send(&pkt).await;
                                    metrics_reader
                                        .srt_retransmits
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    debug!(seq = nak_seq, "SRT retransmit");
                                }
                            }
                        }
                    }
                    Ok(_) => {} // too short, ignore
                    Err(e) => warn!("SrtSender recv error: {}", e),
                }
            }
        });

        // Main send loop
        loop {
            tokio::select! {
                Some(payload) = rx.recv() => {
                    // Fragment if > MTU
                    for chunk in payload.chunks(MAX_UDP_PAYLOAD) {
                        let pkt = encode_data_packet(seq, 96, chunk, false);
                        retransmit_buf.lock().await.insert(seq, pkt.clone());
                        match sock.send(&pkt).await {
                            Ok(_) => {
                                self.metrics.packets_sent
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                self.metrics.pframes_srt
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            Err(e) => {
                                warn!("SRT send error seq={}: {}", seq, e);
                                self.metrics.packets_lost
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        seq = seq.wrapping_add(1);
                    }
                }
                _ = evict_tick.tick() => {
                    retransmit_buf.lock().await.evict_stale();
                }
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// SrtReceiver (skeleton for the relay/decoder side)
// ──────────────────────────────────────────────────────────────────────────────

/// Receives SRT-lite packets, reorders them, and sends ACK/NAK responses.
pub struct SrtReceiver {
    local_addr: SocketAddr,
}

impl SrtReceiver {
    pub fn new(local_addr: SocketAddr) -> Self {
        Self { local_addr }
    }

    /// Returns a channel that yields reassembled payload bytes in order.
    pub async fn run(self, tx: mpsc::Sender<Bytes>) -> anyhow::Result<()> {
        let sock = UdpSocket::bind(self.local_addr).await?;
        let mut received: HashMap<u32, Bytes> = HashMap::new();
        let mut next_expected: u32 = 0;
        let mut buf = vec![0u8; 2048];
        let mut ack_tick = interval(Duration::from_millis(5));

        loop {
            tokio::select! {
                Ok((n, sender)) = sock.recv_from(&mut buf) => {
                    if n < HEADER_SIZE { continue; }
                    let seq = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
                    let flags = buf[4];
                    let payload_len = u16::from_be_bytes([buf[6], buf[7]]) as usize;

                    if flags & FLAG_ACK == 0 && payload_len + HEADER_SIZE <= n {
                        let payload = Bytes::copy_from_slice(&buf[HEADER_SIZE..HEADER_SIZE + payload_len]);
                        received.insert(seq, payload);

                        // Drain in-order packets
                        while let Some(pkt) = received.remove(&next_expected) {
                            let _ = tx.send(pkt).await;
                            next_expected = next_expected.wrapping_add(1);
                        }
                    }
                }
                _ = ack_tick.tick() => {
                    // Build NAK list: any gaps below the highest received seq
                    let max_seen = received.keys().copied().max().unwrap_or(next_expected);
                    let naks: Vec<u32> = (next_expected..max_seen)
                        .filter(|s| !received.contains_key(s))
                        .take(32) // cap NAK list size
                        .collect();

                    let ack_pkt = encode_ack_packet(next_expected.saturating_sub(1), &naks);
                    // Echo back to sender (addr not tracked here — add in production)
                    trace!(ack_up_to = next_expected - 1, naks = naks.len(), "SRT ACK");
                }
            }
        }
    }
}
