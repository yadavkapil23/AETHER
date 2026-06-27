//! # AETHER Encode
//!
//! Encoder trait and implementations for H.264 and stub for H.265.
//!
//! ## Latency Presets
//!
//! H.264 encoders expose a quality-latency tradeoff through lookahead:
//! | Preset     | Lookahead | Added latency | Use case                |
//! |------------|-----------|---------------|-------------------------|
//! | UltraLow   | 0 frames  | ~0ms          | Live streaming <50ms    |
//! | Low        | 3 frames  | ~50ms @ 60fps | Broadcast               |
//! | Balanced   | 10 frames | ~167ms        | VOD / archive           |

use proto::{CodecConfig, CodecType, EncodedFrame, LatencyPreset, RawFrame};

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("codec not supported: {0:?}")]
    UnsupportedCodec(CodecType),
    #[error("encoder not initialised")]
    NotInitialised,
    #[error("invalid frame dimensions: {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    #[error("encode failed: {0}")]
    EncodeFailed(String),
}

/// Synchronous encoder trait.
///
/// Encoding is CPU-bound and runs on a tokio blocking thread via
/// `tokio::task::spawn_blocking`. The trait is synchronous to allow
/// easy wrapping.
pub trait Encoder: Send {
    /// Encodes a single raw frame and returns the compressed output.
    fn encode(&mut self, frame: RawFrame) -> Result<EncodedFrame, EncodeError>;
    /// Forces a keyframe on the next call to `encode`.
    fn request_keyframe(&mut self);
    /// Returns current encoder statistics.
    fn stats(&self) -> EncoderStats;
}

/// Encoder performance statistics.
#[derive(Debug, Clone, Default)]
pub struct EncoderStats {
    pub frames_encoded: u64,
    pub keyframes_encoded: u64,
    pub avg_encode_time_us: u64,
    pub current_bitrate_kbps: u32,
}

/// H.264 software encoder (stub wrapping openh264 or x264).
///
/// # Implementation Status
/// The trait interface and config are fully specified. The actual codec calls
/// are stubbed — they produce synthetic output of the correct shape.
pub struct H264Encoder {
    config: CodecConfig,
    stats: EncoderStats,
    force_keyframe: bool,
    gop_counter: u32,
}

impl H264Encoder {
    pub fn new(config: CodecConfig) -> Result<Self, EncodeError> {
        if config.codec != CodecType::H264 {
            return Err(EncodeError::UnsupportedCodec(config.codec));
        }
        tracing::info!(
            preset = ?config.preset,
            bitrate = config.bitrate_kbps,
            "H264Encoder: initialised (stub)"
        );
        Ok(Self { config, stats: EncoderStats::default(), force_keyframe: true, gop_counter: 0 })
    }

    pub fn ultralow_1080p() -> Result<Self, EncodeError> {
        Self::new(CodecConfig::ultralow_1080p())
    }
}

impl Encoder for H264Encoder {
    fn encode(&mut self, frame: RawFrame) -> Result<EncodedFrame, EncodeError> {
        let is_keyframe = self.force_keyframe || self.gop_counter % self.config.keyframe_interval == 0;
        self.force_keyframe = false;
        self.gop_counter += 1;
        self.stats.frames_encoded += 1;
        if is_keyframe { self.stats.keyframes_encoded += 1; }

        // Stub: produce a synthetic NAL unit of realistic size
        let payload_size = if is_keyframe {
            (self.config.bitrate_kbps * 1000 / 8 / self.config.fps) as usize * 3
        } else {
            (self.config.bitrate_kbps * 1000 / 8 / self.config.fps) as usize
        };
        let data = vec![0u8; payload_size.max(16)];

        Ok(EncodedFrame {
            id: frame.id,
            data,
            is_keyframe,
            pts_us: frame.pts_us,
            codec: self.config.codec,
        })
    }

    fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    fn stats(&self) -> EncoderStats {
        self.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::RawFrame;

    #[test]
    fn first_frame_is_keyframe() {
        let mut enc = H264Encoder::ultralow_1080p().unwrap();
        let frame = RawFrame::synthetic(1920, 1080, 0);
        let encoded = enc.encode(frame).unwrap();
        assert!(encoded.is_keyframe);
    }

    #[test]
    fn request_keyframe_forces_keyframe() {
        let mut enc = H264Encoder::ultralow_1080p().unwrap();
        // Drain the auto-keyframe
        enc.encode(RawFrame::synthetic(1920, 1080, 0)).unwrap();
        // Request forced keyframe
        enc.request_keyframe();
        let encoded = enc.encode(RawFrame::synthetic(1920, 1080, 16_667)).unwrap();
        assert!(encoded.is_keyframe);
    }

    #[test]
    fn p_frames_are_smaller_than_keyframes() {
        let mut enc = H264Encoder::ultralow_1080p().unwrap();
        let kf = enc.encode(RawFrame::synthetic(1920, 1080, 0)).unwrap();
        let pf = enc.encode(RawFrame::synthetic(1920, 1080, 16_667)).unwrap();
        assert!(kf.data.len() > pf.data.len());
    }
}
