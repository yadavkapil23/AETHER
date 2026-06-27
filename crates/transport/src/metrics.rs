//! Atomic transport metrics — counters that every task can update lock-free.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Lock-free counters updated by every transport task.
///
/// All fields use `SeqCst` ordering so any thread reading them sees a
/// consistent snapshot (good enough for telemetry — not a performance path).
#[derive(Debug, Default)]
pub struct TransportMetrics {
    /// Total RTP packets sent (QUIC + SRT combined)
    pub packets_sent: AtomicU64,
    /// Packets reported lost by the receiver (via NACK / SACK gap)
    pub packets_lost: AtomicU64,
    /// Retransmissions triggered by SRT-lite ARQ
    pub srt_retransmits: AtomicU64,
    /// Number of AdaptationDecision changes (SRT↔QUIC transitions)
    pub adaptation_decisions: AtomicU64,
    /// Keyframes sent over QUIC
    pub keyframes_quic: AtomicU64,
    /// P-frames sent over SRT-lite
    pub pframes_srt: AtomicU64,
    /// P-frames promoted to QUIC during high-loss episodes
    pub pframes_promoted_to_quic: AtomicU64,
}

impl TransportMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn loss_rate(&self) -> f32 {
        let sent = self.packets_sent.load(Ordering::SeqCst);
        let lost = self.packets_lost.load(Ordering::SeqCst);
        if sent == 0 {
            return 0.0;
        }
        lost as f32 / sent as f32
    }

    /// Snapshot suitable for logging / Prometheus.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            packets_sent: self.packets_sent.load(Ordering::SeqCst),
            packets_lost: self.packets_lost.load(Ordering::SeqCst),
            srt_retransmits: self.srt_retransmits.load(Ordering::SeqCst),
            adaptation_decisions: self.adaptation_decisions.load(Ordering::SeqCst),
            keyframes_quic: self.keyframes_quic.load(Ordering::SeqCst),
            pframes_srt: self.pframes_srt.load(Ordering::SeqCst),
            pframes_promoted_to_quic: self.pframes_promoted_to_quic.load(Ordering::SeqCst),
        }
    }
}

/// A copyable point-in-time snapshot of [`TransportMetrics`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricsSnapshot {
    pub packets_sent: u64,
    pub packets_lost: u64,
    pub srt_retransmits: u64,
    pub adaptation_decisions: u64,
    pub keyframes_quic: u64,
    pub pframes_srt: u64,
    pub pframes_promoted_to_quic: u64,
}
