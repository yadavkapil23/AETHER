//! # `proto` — AETHER shared protocol types
//!
//! This is the **foundation crate** for the AETHER sub-50 ms live-streaming
//! pipeline.  Every other crate in the workspace imports `proto`; `proto`
//! itself has **zero** internal workspace dependencies.
//!
//! ## Module layout
//! | Module | Purpose |
//! |--------|---------|
//! | [`frame_id`] | Globally-unique, monotonically-increasing frame identifiers |
//! | [`raw_frame`] | Uncompressed video frames straight from the capture device |
//! | [`rtp`] | RTP packet envelope used for media transport |
//! | [`whip_whep`] | WHIP/WHEP HTTP signalling message types |
//! | [`codec`] | Codec selection and latency-preset configuration |
//! | [`pipeline`] | Enumeration of every stage in the AETHER pipeline |
//! | [`encoded_frame`] | Compressed video frames produced by the encoder stage |

#![deny(missing_docs)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use serde::{Deserialize, Serialize};
use bytes::Bytes;

// ── Re-exports ────────────────────────────────────────────────────────────────
pub use frame_id::FrameId;
pub use raw_frame::{PixelFormat, RawFrame};
pub use rtp::RtpPacket;
pub use whip_whep::{WhepRequest, WhepResponse, WhipRequest, WhipResponse};
pub use codec::{CodecConfig, CodecType, LatencyPreset};
pub use pipeline::PipelineStage;
pub use encoded_frame::EncodedFrame;

// ── frame_id ──────────────────────────────────────────────────────────────────

/// Globally-unique, monotonically-increasing identifiers for video frames.
///
/// Using a module keeps the `FRAME_COUNTER` static isolated from the rest of
/// the crate's namespace while still allowing `FrameId` to be re-exported flat.
pub mod frame_id {
    use super::*;
    use std::fmt;

    /// Process-wide counter; wraps at u64::MAX (≈ 584 years at 1 GHz).
    static FRAME_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A unique identifier assigned to each video frame at capture time.
    ///
    /// `FrameId` is a thin `u64` newtype that threads can copy cheaply.  The
    /// value is **strictly increasing within a process**, making it suitable for
    /// ordering, deduplication, and latency attribution across pipeline stages.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct FrameId(pub u64);

    impl FrameId {
        /// Allocate the next frame identifier from the global counter.
        ///
        /// Uses `SeqCst` ordering so that all threads observe the same sequence,
        /// even on weakly-ordered architectures.
        #[inline]
        #[must_use]
        pub fn new() -> Self {
            Self(FRAME_COUNTER.fetch_add(1, Ordering::SeqCst))
        }

        /// Return the raw `u64` value.
        #[inline]
        #[must_use]
        pub fn raw(self) -> u64 {
            self.0
        }
    }

    impl Default for FrameId {
        /// Calls [`FrameId::new`] so that `FrameId::default()` always returns a
        /// *new*, unique identifier rather than a sentinel zero value.
        fn default() -> Self {
            Self::new()
        }
    }

    impl fmt::Display for FrameId {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Frame({})", self.0)
        }
    }
}

// ── raw_frame ─────────────────────────────────────────────────────────────────

/// Types representing an uncompressed video frame as it leaves the capture
/// device and before it enters the encoder.
pub mod raw_frame {
    use super::*;

    /// The pixel layout of a [`RawFrame`] buffer.
    ///
    /// Knowing the layout is required to slice the buffer correctly and to
    /// choose the right encoder input format.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum PixelFormat {
        /// Planar YUV 4:2:0 — the most common camera/encoder format.
        Yuv420p,
        /// Packed RGB with 1 byte per channel — used in synthetic test frames.
        Rgb24,
        /// Semi-planar YUV 4:2:0 — common on mobile / hardware decoders.
        Nv12,
    }

    /// A single uncompressed video frame, reference-counted so it can be
    /// handed to multiple pipeline stages without copying the pixel data.
    ///
    /// The `Arc<[u8]>` payload means cloning a `RawFrame` is O(1); the
    /// underlying pixels are only freed when the last stage drops its handle.
    #[derive(Debug, Clone)]
    pub struct RawFrame {
        /// Globally-unique identifier assigned at capture time.
        pub id: FrameId,
        /// Pixel data in the layout described by `pixel_format`.
        pub data: Arc<[u8]>,
        /// Frame width in pixels.
        pub width: u32,
        /// Frame height in pixels.
        pub height: u32,
        /// Presentation timestamp in **microseconds** since stream start.
        pub pts_us: u64,
        /// Memory layout of `data`.
        pub pixel_format: PixelFormat,
    }

    impl RawFrame {
        /// Create a synthetic frame filled with zeroed bytes, useful for unit
        /// tests and pipeline benchmarks that do not need real camera input.
        ///
        /// The buffer is sized correctly for `PixelFormat::Yuv420p`:
        /// `width * height` luma bytes + `2 * (width/2) * (height/2)` chroma bytes.
        #[must_use]
        pub fn synthetic(width: u32, height: u32, pts_us: u64) -> Self {
            let luma = (width * height) as usize;
            let chroma = 2 * ((width / 2) * (height / 2)) as usize;
            let buf: Arc<[u8]> = vec![0u8; luma + chroma].into();
            Self {
                id: FrameId::new(),
                data: buf,
                width,
                height,
                pts_us,
                pixel_format: PixelFormat::Yuv420p,
            }
        }

        /// Return the expected buffer length for a `Yuv420p` frame of the given
        /// dimensions.  Useful for assertions in downstream crates.
        #[inline]
        #[must_use]
        pub fn yuv420p_size(width: u32, height: u32) -> usize {
            let luma = (width * height) as usize;
            let chroma = 2 * ((width / 2) * (height / 2)) as usize;
            luma + chroma
        }
    }
}

// ── rtp ───────────────────────────────────────────────────────────────────────

/// RTP packet types used to carry compressed media between pipeline stages
/// and across the network.
pub mod rtp {
    use super::*;

    /// A minimal RTP packet envelope conforming to RFC 3550.
    ///
    /// AETHER keeps the header small and avoids heap allocation for the
    /// version/sequence/timestamp fields; only the variable-length `payload`
    /// and the `frame_id` back-reference require owned storage.
    #[derive(Debug, Clone)]
    pub struct RtpPacket {
        /// RTP version — always `2` per RFC 3550.
        pub version: u8,
        /// Payload type identifying the codec (e.g. 96 for H.264).
        pub payload_type: u8,
        /// Monotonically increasing per-SSRC packet counter.
        pub sequence: u16,
        /// Codec clock timestamp (units depend on `payload_type`).
        pub timestamp: u32,
        /// Synchronisation source — unique per media stream.
        pub ssrc: u32,
        /// Back-reference to the [`FrameId`] that produced this packet,
        /// enabling per-frame latency accounting even after packetisation.
        pub frame_id: FrameId,
        /// The encoded payload bytes for this packet.
        pub payload: Bytes,
    }

    impl RtpPacket {
        /// Construct a new RTP packet.  `version` is always set to `2`.
        #[must_use]
        pub fn new(
            payload_type: u8,
            sequence: u16,
            timestamp: u32,
            ssrc: u32,
            frame_id: FrameId,
            payload: Bytes,
        ) -> Self {
            Self {
                version: 2,
                payload_type,
                sequence,
                timestamp,
                ssrc,
                frame_id,
                payload,
            }
        }
    }
}

// ── whip_whep ─────────────────────────────────────────────────────────────────

/// HTTP signalling message types for the WHIP (ingest) and WHEP (egress)
/// protocols used by AETHER's signalling layer.
///
/// WHIP and WHEP are thin HTTP wrappers around SDP offer/answer exchange;
/// these structs are what the HTTP handlers serialise/deserialise.
pub mod whip_whep {
    use super::*;

    /// Sent by the encoder to the AETHER ingest endpoint to initiate a WHIP
    /// session.  Contains an SDP offer and an opaque stream key.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WhipRequest {
        /// SDP offer produced by the encoder's WebRTC stack.
        pub sdp: String,
        /// Opaque credential that identifies and authorises the stream.
        pub stream_key: String,
    }

    /// Server response to a [`WhipRequest`].  Provides the SDP answer and a
    /// resource URL the encoder can use to tear the session down later.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WhipResponse {
        /// Absolute URL of the newly-created WHIP resource (for DELETE).
        pub location: String,
        /// SDP answer from the AETHER server's WebRTC stack.
        pub answer_sdp: String,
    }

    /// Sent by a viewer to the AETHER egress endpoint to initiate a WHEP
    /// session and start receiving the live stream.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WhepRequest {
        /// URL of the WHEP resource the viewer wants to consume.
        pub resource_url: String,
    }

    /// Server response to a [`WhepRequest`].  Returns the SDP answer and any
    /// ICE server configuration required to punch through NAT.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WhepResponse {
        /// SDP answer from the AETHER server's WebRTC stack.
        pub sdp: String,
        /// STUN/TURN server URLs the client should add to its ICE configuration.
        pub ice_servers: Vec<String>,
    }
}

// ── codec ─────────────────────────────────────────────────────────────────────

/// Codec selection and quality/latency preset types for the AETHER encoder
/// stage.
pub mod codec {
    use super::*;

    /// Video codec variants supported by the AETHER encoder.
    ///
    /// Selecting a codec affects both the encoder crate and the RTP
    /// payload-type negotiated during WHIP signalling.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub enum CodecType {
        /// H.264 / AVC — widest device support, lowest encoder complexity.
        H264,
        /// H.265 / HEVC — ~40 % better compression than H.264.
        H265,
        /// VP9 — royalty-free, good browser support.
        Vp9,
        /// AV1 — best compression ratio, highest encoder CPU cost.
        Av1,
    }

    /// Latency vs. quality trade-off preset for the encoder.
    ///
    /// The preset controls the lookahead depth.  A smaller lookahead means
    /// the encoder makes decisions based on fewer future frames, trading
    /// compression efficiency for lower end-to-end latency.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub enum LatencyPreset {
        /// 1-frame lookahead.  Targets < 50 ms glass-to-glass latency.
        UltraLow,
        /// 3-frame lookahead.  Good balance for interactive streams.
        Low,
        /// 10-frame lookahead.  Best quality; use for VOD or high-latency paths.
        Balanced,
    }

    /// Complete configuration for one encoder instance.
    ///
    /// `CodecConfig` is intentionally a plain data struct so it can be cloned
    /// cheaply, serialised to disk, and sent across channel boundaries without
    /// any synchronisation overhead.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CodecConfig {
        /// Which codec to use.
        pub codec: CodecType,
        /// Quality/latency trade-off.
        pub preset: LatencyPreset,
        /// Target output bitrate in kilobits per second.
        pub bitrate_kbps: u32,
        /// Distance between IDR (keyframe) frames in encoded frames.
        pub keyframe_interval: u32,
        /// Output frame width in pixels.
        pub width: u32,
        /// Output frame height in pixels.
        pub height: u32,
        /// Target frame rate.
        pub fps: u32,
    }

    impl CodecConfig {
        /// H.264 at 1080p, 4 Mbps, `UltraLow` preset — the recommended
        /// configuration for interactive streaming under 50 ms latency.
        #[must_use]
        pub fn ultralow_1080p() -> Self {
            Self {
                codec: CodecType::H264,
                preset: LatencyPreset::UltraLow,
                bitrate_kbps: 4_000,
                keyframe_interval: 60,
                width: 1920,
                height: 1080,
                fps: 60,
            }
        }

        /// H.264 at 1080p, 6 Mbps, `Balanced` preset — higher quality at the
        /// cost of ~10 frames of additional latency.
        #[must_use]
        pub fn balanced_1080p() -> Self {
            Self {
                codec: CodecType::H264,
                preset: LatencyPreset::Balanced,
                bitrate_kbps: 6_000,
                keyframe_interval: 120,
                width: 1920,
                height: 1080,
                fps: 60,
            }
        }
    }
}

// ── pipeline ──────────────────────────────────────────────────────────────────

/// Pipeline stage enumeration used for latency accounting and observability.
pub mod pipeline {
    use super::*;
    use std::fmt;

    /// Each variant represents one stage in the AETHER pipeline.
    ///
    /// A frame accumulates timestamps at every stage; the difference between
    /// consecutive timestamps gives per-stage latency, and
    /// `Capture → Complete` gives the true glass-to-glass latency.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[repr(u8)]
    pub enum PipelineStage {
        /// Raw pixels arrive from the capture device (camera / screen grab).
        Capture = 0,
        /// Compressed bitstream produced by the software/hardware encoder.
        Encode = 1,
        /// Compressed data wrapped in RTP packets.
        Packetize = 2,
        /// Packets handed to the OS network stack.
        Send = 3,
        /// Packets received and reassembled on the viewer side.
        Receive = 4,
        /// Raw pixels recovered from the compressed bitstream.
        Decode = 5,
        /// Pixels submitted to the GPU / display compositor.
        Render = 6,
        /// Synthetic marker — stamped when a frame has been fully processed
        /// end-to-end.  Used to record total glass-to-glass latency.
        Complete = 7,
    }

    impl PipelineStage {
        /// Total number of pipeline stages, including [`PipelineStage::Complete`].
        pub const COUNT: usize = 8;

        /// Convert the stage to a `usize` index suitable for array indexing.
        #[inline]
        #[must_use]
        pub fn index(self) -> usize {
            self as usize
        }

        /// Human-readable label for logging and metrics.
        #[must_use]
        pub fn label(self) -> &'static str {
            match self {
                Self::Capture   => "capture",
                Self::Encode    => "encode",
                Self::Packetize => "packetize",
                Self::Send      => "send",
                Self::Receive   => "receive",
                Self::Decode    => "decode",
                Self::Render    => "render",
                Self::Complete  => "complete",
            }
        }
    }

    impl fmt::Display for PipelineStage {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.label())
        }
    }
}

// ── encoded_frame ─────────────────────────────────────────────────────────────

/// Types representing a compressed video frame as it leaves the encoder stage.
pub mod encoded_frame {
    use super::*;

    /// A compressed video frame produced by the encoder stage.
    ///
    /// Unlike [`RawFrame`], `EncodedFrame` owns its data in a plain `Vec<u8>`
    /// because encoded frames are typically consumed once and then discarded
    /// after packetisation; the extra reference-count overhead of `Arc` is not
    /// justified here.
    #[derive(Debug, Clone)]
    pub struct EncodedFrame {
        /// Identifier shared with the originating [`RawFrame`].
        pub id: FrameId,
        /// Compressed payload bytes.
        pub data: Vec<u8>,
        /// `true` if this frame is an IDR / keyframe that can be decoded
        /// independently without prior frames.
        pub is_keyframe: bool,
        /// Presentation timestamp in **microseconds** since stream start,
        /// copied from the source [`RawFrame`].
        pub pts_us: u64,
        /// Codec that produced this bitstream.
        pub codec: CodecType,
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use super::*;

    /// `FrameId::new()` must produce a unique value on every call, even when
    /// called 1 000 times in tight succession on a single thread.
    #[test]
    fn frame_id_uniqueness() {
        let ids: HashSet<u64> = (0..1_000).map(|_| FrameId::new().raw()).collect();
        assert_eq!(ids.len(), 1_000, "FrameId collision detected");
    }

    /// `RawFrame::synthetic` must allocate a buffer whose length matches the
    /// YUV 4:2:0 formula: `W*H + 2*(W/2)*(H/2)`.
    #[test]
    fn synthetic_frame_correct_buffer_size() {
        let (w, h) = (1920u32, 1080u32);
        let frame = RawFrame::synthetic(w, h, 0);
        let expected = RawFrame::yuv420p_size(w, h);
        assert_eq!(
            frame.data.len(),
            expected,
            "synthetic frame buffer size mismatch: got {} expected {}",
            frame.data.len(),
            expected
        );
    }

    /// `RawFrame::synthetic` must tag the buffer as `PixelFormat::Yuv420p`.
    #[test]
    fn synthetic_frame_pixel_format() {
        let frame = RawFrame::synthetic(640, 480, 42);
        assert_eq!(frame.pixel_format, PixelFormat::Yuv420p);
    }

    /// `RtpPacket::new` must always set `version = 2`.
    #[test]
    fn rtp_packet_version_is_two() {
        let pkt = RtpPacket::new(96, 1, 0, 0xDEAD_BEEF, FrameId::new(), Bytes::new());
        assert_eq!(pkt.version, 2);
    }

    /// `PipelineStage::COUNT` must equal the number of variants.
    #[test]
    fn pipeline_stage_count() {
        use PipelineStage::*;
        let all = [Capture, Encode, Packetize, Send, Receive, Decode, Render, Complete];
        assert_eq!(all.len(), PipelineStage::COUNT);
    }

    /// Every `PipelineStage` must produce a non-empty label string.
    #[test]
    fn pipeline_stage_labels_non_empty() {
        use PipelineStage::*;
        let all = [Capture, Encode, Packetize, Send, Receive, Decode, Render, Complete];
        for stage in all {
            assert!(!stage.label().is_empty(), "{stage:?} has empty label");
        }
    }
}
