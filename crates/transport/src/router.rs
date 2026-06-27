//! Frame router: dispatches encoded frames to QUIC or SRT-lite based on the
//! current [`AdaptationDecision`].
//!
//! ## Routing Rules
//!
//! | Frame type | Normal (UseSrt) | High-loss (UseQuic) | ProbeRecovery |
//! |-----------|-----------------|---------------------|---------------|
//! | Keyframe  | QUIC            | QUIC                | QUIC          |
//! | P-frame   | SRT             | QUIC                | QUIC          |
//!
//! Keyframes **always** go to QUIC, regardless of adaptation state.
//! This ensures decoder synchronisation is never compromised.

use std::sync::Arc;

use bytes::Bytes;
use proto::{EncodedFrame, FrameId, PipelineStage};
use telemetry::LatencyTracker;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, trace};

use crate::adaptation::AdaptationDecision;
use crate::metrics::TransportMetrics;

/// Configuration for the frame router.
#[derive(Debug, Clone)]
pub struct RouterConfig {
    /// Channel capacity for the QUIC send queue
    pub quic_queue_depth: usize,
    /// Channel capacity for the SRT send queue
    pub srt_queue_depth: usize,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            quic_queue_depth: 64,
            srt_queue_depth: 256,
        }
    }
}

/// Routes [`EncodedFrame`]s to the appropriate transport path.
///
/// ## Usage
///
/// ```ignore
/// let (router, quic_rx, srt_rx) = FrameRouter::new(config, decision_rx, metrics, tracker);
/// tokio::spawn(quic_path.run_sender(client_config, quic_rx));
/// tokio::spawn(srt_sender.run(srt_rx));
/// tokio::spawn(router.run(encoded_frame_rx));
/// ```
pub struct FrameRouter {
    config: RouterConfig,
    /// Watch receiver: updated every time the probe changes the decision
    decision_rx: watch::Receiver<AdaptationDecision>,
    metrics: Arc<TransportMetrics>,
    tracker: Arc<LatencyTracker>,
    quic_tx: mpsc::Sender<Bytes>,
    srt_tx: mpsc::Sender<Bytes>,
}

impl FrameRouter {
    /// Creates a new router and returns the QUIC and SRT receive ends.
    pub fn new(
        config: RouterConfig,
        decision_rx: watch::Receiver<AdaptationDecision>,
        metrics: Arc<TransportMetrics>,
        tracker: Arc<LatencyTracker>,
    ) -> (Self, mpsc::Receiver<Bytes>, mpsc::Receiver<Bytes>) {
        let (quic_tx, quic_rx) = mpsc::channel(config.quic_queue_depth);
        let (srt_tx, srt_rx) = mpsc::channel(config.srt_queue_depth);
        let router = Self {
            config,
            decision_rx,
            metrics,
            tracker,
            quic_tx,
            srt_tx,
        };
        (router, quic_rx, srt_rx)
    }

    /// Starts the routing loop.
    ///
    /// `frame_rx`: channel from the encoder delivering [`EncodedFrame`]s.
    pub async fn run(mut self, mut frame_rx: mpsc::Receiver<EncodedFrame>) {
        info!("FrameRouter: starting");
        while let Some(frame) = frame_rx.recv().await {
            let decision = *self.decision_rx.borrow();
            self.route_frame(frame, decision).await;
        }
        info!("FrameRouter: encoder channel closed, stopping");
    }

    async fn route_frame(&self, frame: EncodedFrame, decision: AdaptationDecision) {
        // Stamp Packetize stage
        self.tracker.record(frame.id, PipelineStage::Packetize);

        let payload = Bytes::from(frame.data.clone());

        let use_quic = frame.is_keyframe || decision.is_quic();

        if use_quic {
            if !frame.is_keyframe {
                // P-frame promoted due to high loss
                self.metrics
                    .pframes_promoted_to_quic
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            trace!(
                frame_id = %frame.id,
                is_keyframe = frame.is_keyframe,
                path = "QUIC",
                decision = decision.label(),
                "routing frame"
            );
            // Non-blocking send: if QUIC queue is full, drop (backpressure)
            if self.quic_tx.try_send(payload).is_err() {
                debug!("QUIC queue full, dropping frame {}", frame.id);
            }
        } else {
            trace!(
                frame_id = %frame.id,
                path = "SRT",
                "routing frame"
            );
            if self.srt_tx.try_send(payload).is_err() {
                debug!("SRT queue full, dropping frame {}", frame.id);
            }
        }

        self.tracker.record(frame.id, PipelineStage::Send);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proto::{CodecType, EncodedFrame, FrameId};
    use telemetry::LatencyTracker;

    fn make_frame(is_keyframe: bool) -> EncodedFrame {
        EncodedFrame {
            id: FrameId::new(),
            data: vec![0u8; 100],
            is_keyframe,
            pts_us: 0,
            codec: CodecType::H264,
        }
    }

    #[tokio::test]
    async fn keyframes_always_go_to_quic() {
        let metrics = TransportMetrics::new();
        let tracker = Arc::new(LatencyTracker::new());
        let (decision_tx, decision_rx) = watch::channel(AdaptationDecision::UseSrt);
        let (router, mut quic_rx, mut srt_rx) =
            FrameRouter::new(RouterConfig::default(), decision_rx, metrics, tracker.clone());

        let frame = make_frame(true); // keyframe
        let fid = frame.id;
        tracker.begin(fid);
        tracker.record(fid, PipelineStage::Capture);
        tracker.record(fid, PipelineStage::Encode);

        router.route_frame(frame, AdaptationDecision::UseSrt).await;

        // Keyframe must appear on QUIC channel
        assert!(quic_rx.try_recv().is_ok(), "keyframe must go to QUIC");
        assert!(srt_rx.try_recv().is_err(), "keyframe must not go to SRT");
    }

    #[tokio::test]
    async fn p_frames_go_to_srt_when_healthy() {
        let metrics = TransportMetrics::new();
        let tracker = Arc::new(LatencyTracker::new());
        let (_tx, decision_rx) = watch::channel(AdaptationDecision::UseSrt);
        let (router, mut quic_rx, mut srt_rx) =
            FrameRouter::new(RouterConfig::default(), decision_rx, metrics, tracker.clone());

        let frame = make_frame(false); // P-frame
        let fid = frame.id;
        tracker.begin(fid);
        tracker.record(fid, PipelineStage::Capture);
        tracker.record(fid, PipelineStage::Encode);

        router.route_frame(frame, AdaptationDecision::UseSrt).await;

        assert!(srt_rx.try_recv().is_ok(), "P-frame must go to SRT");
        assert!(quic_rx.try_recv().is_err(), "P-frame must not go to QUIC");
    }

    #[tokio::test]
    async fn p_frames_promoted_to_quic_on_high_loss() {
        let metrics = TransportMetrics::new();
        let tracker = Arc::new(LatencyTracker::new());
        let (_tx, decision_rx) = watch::channel(AdaptationDecision::UseQuic);
        let (router, mut quic_rx, mut srt_rx) =
            FrameRouter::new(RouterConfig::default(), decision_rx, metrics.clone(), tracker.clone());

        let frame = make_frame(false);
        let fid = frame.id;
        tracker.begin(fid);
        tracker.record(fid, PipelineStage::Capture);
        tracker.record(fid, PipelineStage::Encode);

        router.route_frame(frame, AdaptationDecision::UseQuic).await;

        assert!(quic_rx.try_recv().is_ok(), "P-frame must be promoted to QUIC");
        assert!(srt_rx.try_recv().is_err());
        assert_eq!(
            metrics.pframes_promoted_to_quic.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }
}
