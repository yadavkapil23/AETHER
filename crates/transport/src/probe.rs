//! Network probe: measures RTT, loss rate, and jitter every 16ms.
//!
//! The probe runs as a background tokio task. It sends a small UDP ping packet
//! and listens for a reflected echo. RTT = round-trip time of the echo.
//! Loss rate = fraction of pings with no echo within the timeout window.
//!
//! In a real deployment the probe would use the same socket as the data path.
//! Here we use a separate lightweight UDP probe socket for clarity.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::time::{interval, timeout};
use tracing::{debug, warn};

use crate::adaptation::{AdaptationDecision, AdaptationState};
use crate::metrics::TransportMetrics;

/// The network conditions measured during one probe cycle.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct NetworkProbe {
    /// Round-trip time (ping → echo)
    pub rtt: Duration,
    /// Fraction of packets lost in the last probe window (0.0 – 1.0)
    pub loss_rate: f32,
    /// RTT variance (simple |current_rtt - ema_rtt| estimate)
    pub jitter: Duration,
}

impl Default for NetworkProbe {
    fn default() -> Self {
        Self {
            rtt: Duration::from_millis(10),
            loss_rate: 0.0,
            jitter: Duration::from_millis(1),
        }
    }
}

/// Configuration for the probe task.
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    /// Target address for echo probes (should be a responsive UDP echo server)
    pub remote_addr: SocketAddr,
    /// How often to send a probe packet
    pub probe_interval: Duration,
    /// How long to wait for an echo before counting as lost
    pub echo_timeout: Duration,
    /// Window size for loss rate computation
    pub window_size: usize,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            // Loopback for testing; replace with remote relay address in production
            remote_addr: "127.0.0.1:9000".parse().unwrap(),
            probe_interval: Duration::from_millis(16), // 16ms feedback loop
            echo_timeout: Duration::from_millis(50),
            window_size: 32,
        }
    }
}

/// Runs the 16ms network probe loop.
///
/// Publishes the current [`AdaptationDecision`] on a watch channel so that
/// the [`FrameRouter`] can read it without locking.
pub struct ProbeTask {
    config: ProbeConfig,
    metrics: Arc<TransportMetrics>,
    /// Sends updated decisions to the router
    decision_tx: watch::Sender<AdaptationDecision>,
    /// Latest probe result (readable by anyone with a clone of this task)
    probe_tx: watch::Sender<NetworkProbe>,
}

impl ProbeTask {
    /// Creates a ProbeTask and returns it along with receivers for the decision
    /// and latest probe result.
    pub fn new(
        config: ProbeConfig,
        metrics: Arc<TransportMetrics>,
    ) -> (
        Self,
        watch::Receiver<AdaptationDecision>,
        watch::Receiver<NetworkProbe>,
    ) {
        let (decision_tx, decision_rx) = watch::channel(AdaptationDecision::UseSrt);
        let (probe_tx, probe_rx) = watch::channel(NetworkProbe::default());
        let task = Self {
            config,
            metrics,
            decision_tx,
            probe_tx,
        };
        (task, decision_rx, probe_rx)
    }

    /// Runs the probe loop forever. Spawn this with `tokio::spawn`.
    pub async fn run(self) {
        let mut state = AdaptationState::new();
        let mut tick = interval(self.config.probe_interval);
        // Sliding window of booleans: true = received echo
        let mut window: Vec<bool> = vec![true; self.config.window_size];
        let mut window_pos = 0usize;
        let mut ema_rtt = Duration::from_millis(10);
        let alpha = 0.125_f64; // EWMA smoothing factor (RFC 6298)

        // Bind a local UDP socket for probing
        let sock = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                warn!("ProbeTask: failed to bind UDP socket: {} — using synthetic probes", e);
                // Fall back to synthetic (simulated) probing
                self.run_synthetic(state).await;
                return;
            }
        };

        loop {
            tick.tick().await;

            let start = Instant::now();
            let send_result = sock
                .send_to(b"AETHER_PROBE", self.config.remote_addr)
                .await;

            let (received, rtt) = if send_result.is_ok() {
                let mut buf = [0u8; 64];
                match timeout(self.config.echo_timeout, sock.recv_from(&mut buf)).await {
                    Ok(Ok(_)) => (true, start.elapsed()),
                    _ => (false, self.config.echo_timeout),
                }
            } else {
                (false, self.config.echo_timeout)
            };

            // Update sliding window
            window[window_pos % self.config.window_size] = received;
            window_pos = window_pos.wrapping_add(1);

            // Compute loss rate
            let losses = window.iter().filter(|&&r| !r).count();
            let loss_rate = losses as f32 / self.config.window_size as f32;

            // EWMA RTT and jitter
            let rtt_secs = rtt.as_secs_f64();
            let ema_secs = ema_rtt.as_secs_f64();
            let new_ema = (1.0 - alpha) * ema_secs + alpha * rtt_secs;
            let jitter_secs = (rtt_secs - new_ema).abs();
            ema_rtt = Duration::from_secs_f64(new_ema);

            let probe = NetworkProbe {
                rtt: ema_rtt,
                loss_rate,
                jitter: Duration::from_secs_f64(jitter_secs),
            };

            debug!(
                rtt_ms = probe.rtt.as_millis(),
                loss_pct = probe.loss_rate * 100.0,
                jitter_us = probe.jitter.as_micros(),
                "probe"
            );

            let changed = state.update(&probe);
            if changed {
                self.metrics
                    .adaptation_decisions
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let _ = self.decision_tx.send(state.decision);
            }
            let _ = self.probe_tx.send(probe);
        }
    }

    /// Synthetic probe mode: generates realistic simulated network conditions.
    ///
    /// Used when no real UDP echo server is available (tests, demos).
    /// Simulates a base RTT of 8ms with occasional loss spikes.
    async fn run_synthetic(self, mut state: AdaptationState) {
        use rand::Rng;
        let mut tick = interval(self.config.probe_interval);
        let mut cycle = 0u64;

        loop {
            tick.tick().await;
            cycle += 1;

            // Every 200 probes (3.2s) simulate a 500ms loss episode
            let in_loss_episode = (cycle % 200) < 30;
            let loss_rate = if in_loss_episode {
                rand::thread_rng().gen_range(0.005..0.02_f32) // 0.5–2% loss
            } else {
                rand::thread_rng().gen_range(0.0..0.001_f32) // near-zero
            };

            let base_rtt = if in_loss_episode {
                Duration::from_millis(rand::thread_rng().gen_range(15..40))
            } else {
                Duration::from_millis(rand::thread_rng().gen_range(4..10))
            };

            let probe = NetworkProbe {
                rtt: base_rtt,
                loss_rate,
                jitter: Duration::from_micros(rand::thread_rng().gen_range(100..2000)),
            };

            let changed = state.update(&probe);
            if changed {
                self.metrics
                    .adaptation_decisions
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let _ = self.decision_tx.send(state.decision);
            }
            let _ = self.probe_tx.send(probe);
        }
    }
}
