//! # AETHER Transport Layer
//!
//! Hybrid QUIC + SRT-lite transport with a 16ms adaptive feedback loop.
//!
//! ## Design Rationale
//!
//! **Why not pure QUIC?**
//! QUIC has head-of-line blocking *within a stream*: if a keyframe packet is lost,
//! all subsequent P-frame packets in the same stream stall until retransmission.
//! For live video, this adds 1–2 RTT of latency on every loss event.
//!
//! **Why not pure SRT?**
//! SRT uses selective retransmission but has no reliability guarantee — under heavy
//! loss (>2%) it degrades gracefully by dropping, which is unacceptable for keyframes.
//!
//! **The hybrid solution:**
//! - Keyframes travel over QUIC (reliable, ordered).
//! - P-frames travel over SRT-lite (fast, selective ARQ, drop-tolerant).
//! - A 16ms [`ProbeTask`] measures network conditions and drives [`AdaptationDecision`].
//! - When SRT loss crosses 0.5%, all frames are promoted to QUIC.
//! - When conditions recover (RTT < 8ms, loss < 0.1% for ≥3 consecutive probes),
//!   P-frames return to SRT-lite.
//!
//! ## SRT-lite
//! We implement our own lightweight SRT instead of binding to the C library.
//! The implementation: UDP sockets + sequence numbers + selective ACK (SACK) +
//! a 2ms retransmission timer. No encryption, no handshake protocol — those are
//! handled by the QUIC connection used for the control plane.

pub mod adaptation;
pub mod metrics;
pub mod probe;
pub mod quic_path;
pub mod router;
pub mod srt_lite;
pub mod tls;

pub use adaptation::{AdaptationDecision, AdaptationState};
pub use metrics::TransportMetrics;
pub use probe::{NetworkProbe, ProbeTask};
pub use router::{FrameRouter, RouterConfig};
