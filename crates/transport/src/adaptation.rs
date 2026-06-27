//! Adaptation logic: decides whether to route frames via QUIC or SRT-lite.
//!
//! ## Decision Algorithm
//!
//! ```text
//! State machine:
//!
//!   ┌──────────────────────────────────────────────────────────────────┐
//!   │                                                                  │
//!   │   UseSrt ────[loss > 0.5%]────→ UseQuic                        │
//!   │      ↑                              │                           │
//!   │      └──[loss < 0.1% for 3 probes]──┘                          │
//!   │                                                                  │
//!   │   ProbeRecovery: intermediate state between UseQuic → UseSrt   │
//!   │   Entered after UseQuic, exited after 3 clean probes.           │
//!   └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Hysteresis prevents flapping: we require **3 consecutive clean probes**
//! before transitioning back to SRT from either UseQuic or ProbeRecovery.
//! This means the minimum time in QUIC mode after a loss event is 3×16ms = 48ms.

use std::time::Duration;

use tracing::{info, warn};

use crate::probe::NetworkProbe;

/// The current transport path decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AdaptationDecision {
    /// All frames use SRT-lite (low latency, best-effort reliable)
    UseSrt,
    /// All frames use QUIC (reliable, slightly higher latency under load)
    UseQuic,
    /// Transitioning back from QUIC to SRT — still on QUIC until probes confirm
    /// stability
    ProbeRecovery {
        /// How many consecutive clean probes seen so far
        clean_probes: u8,
    },
}

impl AdaptationDecision {
    /// Returns the human-readable name of the current decision.
    pub fn label(self) -> &'static str {
        match self {
            Self::UseSrt => "SRT",
            Self::UseQuic => "QUIC",
            Self::ProbeRecovery { .. } => "PROBE_RECOVERY",
        }
    }

    /// Returns true if frames should currently be sent over QUIC.
    pub fn is_quic(self) -> bool {
        !matches!(self, Self::UseSrt)
    }
}

// Thresholds
const LOSS_PROMOTE_THRESHOLD: f32 = 0.005; // 0.5% → switch to QUIC
const LOSS_DEMOTE_THRESHOLD: f32 = 0.001; // 0.1% → candidate for SRT return
const RTT_DEMOTE_THRESHOLD: Duration = Duration::from_millis(8); // RTT must be low
const CLEAN_PROBES_REQUIRED: u8 = 3; // hysteresis: 3 × 16ms = 48ms

/// Mutable adaptation state that is updated on every probe cycle.
#[derive(Debug, Clone)]
pub struct AdaptationState {
    pub decision: AdaptationDecision,
    /// Running sum of losses over the last probe window (for smoothing)
    probe_count: u64,
}

impl Default for AdaptationState {
    fn default() -> Self {
        Self {
            decision: AdaptationDecision::UseSrt,
            probe_count: 0,
        }
    }
}

impl AdaptationState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingests a [`NetworkProbe`] and potentially transitions state.
    ///
    /// Returns `true` if the decision changed (useful for emitting metrics).
    pub fn update(&mut self, probe: &NetworkProbe) -> bool {
        self.probe_count += 1;
        let prev = self.decision;

        self.decision = match self.decision {
            AdaptationDecision::UseSrt => {
                if probe.loss_rate > LOSS_PROMOTE_THRESHOLD {
                    warn!(
                        loss_pct = probe.loss_rate * 100.0,
                        rtt_ms = probe.rtt.as_millis(),
                        "Loss threshold exceeded — promoting to QUIC"
                    );
                    AdaptationDecision::UseQuic
                } else {
                    AdaptationDecision::UseSrt
                }
            }

            AdaptationDecision::UseQuic => {
                if probe.loss_rate < LOSS_DEMOTE_THRESHOLD && probe.rtt < RTT_DEMOTE_THRESHOLD {
                    info!("Network stabilised — entering ProbeRecovery");
                    AdaptationDecision::ProbeRecovery { clean_probes: 1 }
                } else {
                    AdaptationDecision::UseQuic
                }
            }

            AdaptationDecision::ProbeRecovery { clean_probes } => {
                if probe.loss_rate > LOSS_PROMOTE_THRESHOLD {
                    // Relapse: back to QUIC, reset counter
                    warn!("Loss spike during ProbeRecovery — returning to QUIC");
                    AdaptationDecision::UseQuic
                } else if probe.loss_rate < LOSS_DEMOTE_THRESHOLD && probe.rtt < RTT_DEMOTE_THRESHOLD {
                    let next = clean_probes + 1;
                    if next >= CLEAN_PROBES_REQUIRED {
                        info!(
                            probes = next,
                            "Hysteresis satisfied — demoting to SRT"
                        );
                        AdaptationDecision::UseSrt
                    } else {
                        AdaptationDecision::ProbeRecovery { clean_probes: next }
                    }
                } else {
                    // Conditions are marginal but not bad — stay in recovery
                    AdaptationDecision::ProbeRecovery { clean_probes }
                }
            }
        };

        self.decision != prev
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::NetworkProbe;

    fn clean_probe() -> NetworkProbe {
        NetworkProbe {
            rtt: Duration::from_millis(4),
            loss_rate: 0.0,
            jitter: Duration::from_micros(500),
        }
    }

    fn lossy_probe() -> NetworkProbe {
        NetworkProbe {
            rtt: Duration::from_millis(20),
            loss_rate: 0.01, // 1% — above threshold
            jitter: Duration::from_millis(2),
        }
    }

    #[test]
    fn test_srt_to_quic_on_loss() {
        let mut state = AdaptationState::new();
        let changed = state.update(&lossy_probe());
        assert!(changed);
        assert_eq!(state.decision, AdaptationDecision::UseQuic);
    }

    #[test]
    fn test_hysteresis_requires_three_clean_probes() {
        let mut state = AdaptationState::new();
        state.update(&lossy_probe());
        assert_eq!(state.decision, AdaptationDecision::UseQuic);

        // 1st clean probe → ProbeRecovery(1)
        state.update(&clean_probe());
        assert!(matches!(
            state.decision,
            AdaptationDecision::ProbeRecovery { clean_probes: 1 }
        ));

        // 2nd → ProbeRecovery(2)
        state.update(&clean_probe());
        assert!(matches!(
            state.decision,
            AdaptationDecision::ProbeRecovery { clean_probes: 2 }
        ));

        // 3rd → UseSrt
        state.update(&clean_probe());
        assert_eq!(state.decision, AdaptationDecision::UseSrt);
    }

    #[test]
    fn test_relapse_resets_recovery() {
        let mut state = AdaptationState::new();
        state.update(&lossy_probe()); // → UseQuic
        state.update(&clean_probe()); // → ProbeRecovery(1)
        // Sudden loss spike → back to QUIC
        state.update(&lossy_probe());
        assert_eq!(state.decision, AdaptationDecision::UseQuic);
    }
}
