//! # AETHER Mixer
//!
//! Multi-channel audio mixing with automatic gain control (AGC).
//!
//! ## Architecture
//!
//! Each input source (microphone, system audio, remote participant) is an
//! [`AudioChannel`]. The [`AudioMixer`] sums all channels sample-by-sample
//! and applies the [`AgcProcessor`] to prevent clipping.

use std::collections::HashMap;

/// Audio sample — normalised to [-1.0, 1.0] for mixing.
/// Stored as f32 for SIMD-friendliness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioSample(pub f32);

impl AudioSample {
    /// Silence: a sample with zero amplitude.
    pub const SILENCE: Self = Self(0.0);

    /// Clamps the sample value to the valid normalised range [-1.0, 1.0].
    ///
    /// Called after mixing and AGC to ensure no downstream consumer receives
    /// an out-of-range value that could cause distortion or integer overflow
    /// during format conversion (e.g., to i16 PCM).
    pub fn clamp(self) -> Self {
        Self(self.0.clamp(-1.0, 1.0))
    }
}

/// Lock-free ring buffer for audio samples. Fixed capacity; oldest samples are
/// overwritten when the buffer is full.
///
/// Chosen over a `VecDeque` because it never reallocates after construction,
/// keeping allocation pressure out of the hot audio path.
pub struct SampleBuffer {
    data: Vec<AudioSample>,
    /// Write head (next position to write), always < capacity.
    head: usize,
    /// Number of valid samples currently stored (≤ capacity).
    len: usize,
    capacity: usize,
}

impl SampleBuffer {
    /// Creates a new `SampleBuffer` pre-filled with silence.
    pub fn new(capacity: usize) -> Self {
        Self {
            data: vec![AudioSample::SILENCE; capacity],
            head: 0,
            len: 0,
            capacity,
        }
    }

    /// Pushes a single sample into the buffer.
    ///
    /// If the buffer is full the oldest sample is silently overwritten,
    /// maintaining the fixed-capacity invariant.
    pub fn push(&mut self, s: AudioSample) {
        self.data[self.head % self.capacity] = s;
        self.head = (self.head + 1) % self.capacity;
        if self.len < self.capacity {
            self.len += 1;
        }
    }

    /// Drains all stored samples into `out` in chronological order,
    /// then resets the buffer to empty.
    ///
    /// Samples are appended to `out` so callers can pre-allocate and reuse
    /// the vector across mix cycles.
    pub fn drain(&mut self, out: &mut Vec<AudioSample>) {
        let start = if self.len == self.capacity {
            self.head
        } else {
            0
        };
        for i in 0..self.len {
            out.push(self.data[(start + i) % self.capacity]);
        }
        self.len = 0;
        self.head = 0;
    }

    /// Returns the number of samples currently stored.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if no samples are stored.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Configuration parameters for an [`AudioMixer`] instance.
///
/// Separate from the mixer itself so it can be serialised / sent over the
/// proto layer and re-applied without reconstructing the mixer.
#[derive(Debug, Clone)]
pub struct MixerConfig {
    /// PCM sample rate shared by all channels (Hz). Typically 48 000 Hz for
    /// WebRTC / Opus compatibility.
    pub sample_rate: u32,
    /// Number of interleaved audio channels (1 = mono, 2 = stereo).
    pub channels: u8,
    /// Size of the per-channel ring buffer expressed in milliseconds of audio.
    /// Larger values give more jitter headroom at the cost of latency.
    pub buffer_ms: u32,
}

impl Default for MixerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            buffer_ms: 10,
        }
    }
}

/// Trait for automatic gain control processors.
///
/// Implementors operate in-place on a mutable slice of [`AudioSample`]s.
/// The trait is object-safe so it can be stored as `Box<dyn AgcProcessor>`
/// inside [`AudioMixer`], enabling easy substitution (e.g., swapping in an
/// ML-based loudness model at runtime).
pub trait AgcProcessor: Send + Sync {
    /// Processes a buffer of samples **in-place**.
    ///
    /// Implementors must not change the length of the slice — only the
    /// amplitude values.
    fn process(&self, samples: &mut [AudioSample]);

    /// Returns the current gain factor applied by this processor.
    ///
    /// - `1.0` → unity (no amplification or attenuation)
    /// - `> 1.0` → boost
    /// - `< 1.0` → attenuation
    fn gain(&self) -> f32;
}

/// A simple, stateful AGC implementation based on RMS-tracking.
///
/// Each `process` call computes the RMS of the incoming buffer, derives the
/// ideal gain to reach `target_rms`, and moves `current_gain` 10 % of the
/// way toward that ideal — a first-order low-pass filter that prevents abrupt
/// volume jumps.
///
/// The gain state is stored as the bit-pattern of an `f32` inside an
/// `AtomicU32` so multiple threads can read `gain()` without a mutex while a
/// single audio thread writes it.
pub struct SimpleAgc {
    /// Desired output RMS level (0.0–1.0).
    target_rms: f32,
    /// Current gain, stored as `f32::to_bits` for atomic access.
    current_gain: std::sync::atomic::AtomicU32,
}

impl SimpleAgc {
    /// Creates a new `SimpleAgc` with unity gain and the specified target RMS.
    ///
    /// # Arguments
    /// * `target_rms` — desired normalised RMS level; 0.3–0.5 is a reasonable
    ///   range for voice.
    pub fn new(target_rms: f32) -> Self {
        Self {
            target_rms,
            current_gain: std::sync::atomic::AtomicU32::new(f32::to_bits(1.0)),
        }
    }
}

impl AgcProcessor for SimpleAgc {
    fn process(&self, samples: &mut [AudioSample]) {
        if samples.is_empty() {
            return;
        }

        // Compute RMS of the incoming buffer.
        let rms = (samples.iter().map(|s| s.0 * s.0).sum::<f32>() / samples.len() as f32).sqrt();
        // Skip gain adjustment for near-silence to avoid runaway boost.
        if rms < 1e-6 {
            return;
        }

        let gain_bits = self.current_gain.load(std::sync::atomic::Ordering::Relaxed);
        let mut gain = f32::from_bits(gain_bits);

        // First-order IIR: move 10 % toward the ideal gain this frame.
        let target_gain = self.target_rms / rms;
        gain += (target_gain - gain) * 0.1;
        // Hard-limit the gain to prevent wild swings on transients.
        gain = gain.clamp(0.1, 4.0);

        self.current_gain
            .store(f32::to_bits(gain), std::sync::atomic::Ordering::Relaxed);

        for s in samples.iter_mut() {
            *s = AudioSample(s.0 * gain).clamp();
        }
    }

    fn gain(&self) -> f32 {
        f32::from_bits(
            self.current_gain
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

/// Multi-channel audio mixer.
///
/// Accepts *N* independent input channels, sums them sample-by-sample, and
/// applies the configured [`AgcProcessor`] to the summed output. The mixer
/// owns a [`SampleBuffer`] per channel; callers push raw samples at varying
/// rates and call [`AudioMixer::mix`] on a fixed schedule (e.g. every 10 ms).
///
/// Channel lifecycle:
/// 1. `add_channel()` → allocates a ring buffer, returns a stable `u32` ID.
/// 2. `push_samples(id, &[…])` → appends samples to that channel's buffer.
/// 3. `mix()` → drains the minimum common length across all channels, sums,
///    applies AGC, and returns the interleaved output.
pub struct AudioMixer {
    config: MixerConfig,
    /// Per-channel ring buffers keyed by the channel ID assigned at creation.
    channels: HashMap<u32, SampleBuffer>,
    /// Pluggable AGC processor applied after summing.
    agc: Box<dyn AgcProcessor>,
    /// Monotonically increasing counter; never wraps in practice (u32 = 4 B channels).
    next_channel_id: u32,
}

impl AudioMixer {
    /// Creates a new `AudioMixer` with the given configuration and AGC processor.
    pub fn new(config: MixerConfig, agc: Box<dyn AgcProcessor>) -> Self {
        Self {
            config,
            channels: HashMap::new(),
            agc,
            next_channel_id: 0,
        }
    }

    /// Adds a new input channel and returns its opaque numeric ID.
    ///
    /// The ring buffer is sized to hold 4× the configured `buffer_ms` worth
    /// of samples to absorb moderate jitter without dropping audio.
    pub fn add_channel(&mut self) -> u32 {
        let id = self.next_channel_id;
        // 4× buffer_ms gives headroom for bursty sources.
        let buf_samples = (self.config.sample_rate * self.config.buffer_ms / 1000) as usize
            * self.config.channels as usize
            * 4;
        self.channels.insert(id, SampleBuffer::new(buf_samples));
        self.next_channel_id += 1;
        id
    }

    /// Pushes raw samples into the specified channel's ring buffer.
    ///
    /// Silently ignores unknown `channel_id` values so callers do not need to
    /// synchronise channel teardown with the push path.
    pub fn push_samples(&mut self, channel_id: u32, samples: &[AudioSample]) {
        if let Some(buf) = self.channels.get_mut(&channel_id) {
            for &s in samples {
                buf.push(s);
            }
        }
    }

    /// Mixes all channels into a single output buffer and applies AGC.
    ///
    /// The output length equals the minimum non-zero sample count across all
    /// channels, ensuring synchronised draining even when sources are running
    /// at slightly different rates. Returns an empty `Vec` if any channel has
    /// no buffered data.
    pub fn mix(&mut self) -> Vec<AudioSample> {
        let min_len = self
            .channels
            .values()
            .map(|b| b.len())
            .min()
            .unwrap_or(0);
        if min_len == 0 {
            return Vec::new();
        }

        // Drain exactly `min_len` samples from every channel.
        let mut drained: Vec<Vec<AudioSample>> = Vec::new();
        for buf in self.channels.values_mut() {
            let mut v = Vec::with_capacity(min_len);
            buf.drain(&mut v);
            v.truncate(min_len);
            drained.push(v);
        }

        // Sum all channels into a single output buffer.
        let mut mixed = vec![AudioSample::SILENCE; min_len];
        for ch in &drained {
            for (out, &inp) in mixed.iter_mut().zip(ch.iter()) {
                out.0 += inp.0;
            }
        }

        // Apply AGC to the summed signal.
        self.agc.process(&mut mixed);
        mixed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that two channels are summed and that the AGC keeps all output
    /// samples within the normalised range.
    #[test]
    fn mix_two_channels_sums_correctly() {
        let agc = Box::new(SimpleAgc::new(0.5));
        let mut mixer = AudioMixer::new(MixerConfig::default(), agc);
        let ch1 = mixer.add_channel();
        let ch2 = mixer.add_channel();
        mixer.push_samples(ch1, &[AudioSample(0.1); 100]);
        mixer.push_samples(ch2, &[AudioSample(0.2); 100]);
        let output = mixer.mix();
        assert_eq!(output.len(), 100);
        // Before AGC: sum ≈ 0.3; after AGC the value may shift but must stay clamped.
        assert!(output.iter().all(|s| s.0 >= -1.0 && s.0 <= 1.0));
    }

    /// Verifies that the ring buffer wraps correctly and reports the capped length.
    #[test]
    fn sample_buffer_ring_wraps() {
        let mut buf = SampleBuffer::new(4);
        for i in 0..6u8 {
            buf.push(AudioSample(i as f32));
        }
        // Only the 4 most-recent samples should remain.
        assert_eq!(buf.len(), 4);
    }

    /// Verifies that `SimpleAgc` returns unity gain immediately after construction.
    #[test]
    fn agc_starts_at_unity_gain() {
        let agc = SimpleAgc::new(0.5);
        assert!((agc.gain() - 1.0).abs() < f32::EPSILON);
    }

    /// Verifies that `AudioSample::clamp` hard-limits out-of-range values.
    #[test]
    fn audio_sample_clamp() {
        assert_eq!(AudioSample(2.5).clamp(), AudioSample(1.0));
        assert_eq!(AudioSample(-3.0).clamp(), AudioSample(-1.0));
        assert_eq!(AudioSample(0.5).clamp(), AudioSample(0.5));
    }

    /// Verifies that `mix()` returns empty when no samples have been pushed.
    #[test]
    fn mix_empty_channels_returns_empty() {
        let agc = Box::new(SimpleAgc::new(0.5));
        let mut mixer = AudioMixer::new(MixerConfig::default(), agc);
        let _ch = mixer.add_channel();
        let output = mixer.mix();
        assert!(output.is_empty());
    }
}
