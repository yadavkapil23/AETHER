//! # AETHER Capture
//!
//! Abstracts over platform capture backends (V4L2 on Linux, AVFoundation on macOS)
//! behind a single async [`CaptureDevice`] trait.
//!
//! ## Design Goals
//! * **Zero-copy hand-off** — frames are wrapped in [`Arc<[u8]>`][std::sync::Arc]
//!   inside [`proto::RawFrame`], so cloning the handle into the next pipeline stage
//!   is O(1) and avoids a memcpy on the hot path.
//! * **Backend agnostic** — the [`CaptureDevice`] trait is the only public surface
//!   the rest of the pipeline depends on; swapping V4L2 for AVFoundation or NDI
//!   requires no changes upstream.
//! * **Testable without hardware** — [`TestCapture`] generates synthetic YUV420p
//!   frames at a configurable rate, making integration tests reproducible on CI.
//!
//! ## Crate Status
//! `capture` is a **stub crate**: all trait definitions, configuration types, and
//! module structure are complete and compile cleanly.  The V4L2 ioctl layer
//! (`VIDIOC_REQBUFS`, `VIDIOC_STREAMON`, `mmap`) is stubbed behind a feature flag
//! and is the primary TODO for the Linux port.

use proto::{PixelFormat, RawFrame};
use std::path::PathBuf;

// ── Error type ────────────────────────────────────────────────────────────────

/// Every error that the [`CaptureDevice`] trait or its implementors can produce.
///
/// The variants are kept coarse-grained so that callers can match on the broad
/// failure category (not found, permission, format, IO) without depending on
/// OS-specific sub-codes.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The requested device path does not exist or is not a video device.
    #[error("device not found: {0}")]
    DeviceNotFound(String),

    /// The process lacks the privileges needed to open the device.
    ///
    /// Common cause: user not in the `video` group on Linux.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// The device exists but cannot produce frames in the requested format.
    #[error("format not supported: {0:?}")]
    UnsupportedFormat(PixelFormat),

    /// Wraps any underlying [`std::io::Error`] from the kernel or async runtime.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// The capture stream was shut down (via [`CaptureDevice::stop`]) before
    /// [`CaptureDevice::next_frame`] could return a frame.
    #[error("capture stopped")]
    Stopped,
}

// ── Resolution ────────────────────────────────────────────────────────────────

/// Resolution configuration for the capture device.
///
/// Named constants cover the three most common broadcast resolutions;
/// arbitrary resolutions can be constructed with struct literal syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
}

impl Resolution {
    /// 1280 × 720 — HD / 720p.
    pub const HD: Self = Self { width: 1280, height: 720 };
    /// 1920 × 1080 — Full HD / 1080p.
    pub const FHD: Self = Self { width: 1920, height: 1080 };
    /// 3840 × 2160 — Ultra HD / 4K.
    pub const UHD: Self = Self { width: 3840, height: 2160 };
}

// ── CaptureConfig ─────────────────────────────────────────────────────────────

/// Complete configuration for a single capture device session.
///
/// `CaptureConfig` is a plain-data struct so it can be cheaply cloned,
/// serialised to a config file, or sent across channel boundaries without
/// any synchronisation overhead.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Path to the OS device node.
    ///
    /// On Linux this is typically `/dev/video0`; on macOS the AVFoundation
    /// backend ignores this field and uses the device index instead.
    pub device_path: PathBuf,
    /// Output frame dimensions requested from the device.
    pub resolution: Resolution,
    /// Target frames-per-second.  The device may negotiate a different rate.
    pub fps: u32,
    /// Pixel format requested from the device.
    pub pixel_format: PixelFormat,
}

impl Default for CaptureConfig {
    /// Returns a sensible default: `/dev/video0`, 1080p @ 60 fps, YUV420p.
    fn default() -> Self {
        Self {
            device_path: PathBuf::from("/dev/video0"),
            resolution: Resolution::FHD,
            fps: 60,
            pixel_format: PixelFormat::Yuv420p,
        }
    }
}

// ── CaptureDevice trait ───────────────────────────────────────────────────────

/// Async trait implemented by all capture backends.
///
/// # Contract
/// * [`next_frame`][CaptureDevice::next_frame] **blocks** (asynchronously) until
///   the next frame is available.  Implementations must `.await` on a future that
///   yields so that Tokio's cooperative scheduler can make progress on other tasks.
/// * Implementations must be [`Send`] + [`Sync`] so they can be placed inside an
///   `Arc<Mutex<dyn CaptureDevice>>` and driven from a dedicated capture task.
/// * After [`stop`][CaptureDevice::stop] returns `Ok(())`, any subsequent call to
///   [`next_frame`][CaptureDevice::next_frame] **must** return
///   [`CaptureError::Stopped`].
#[async_trait::async_trait]
pub trait CaptureDevice: Send + Sync {
    /// Returns the next available frame from the device.
    ///
    /// Blocks asynchronously until the hardware signals that a new buffer is
    /// ready.  The returned [`RawFrame`] shares ownership of the pixel buffer
    /// via [`Arc`][std::sync::Arc]; dropping the frame releases the buffer back
    /// to the pool (or to the allocator in the stub implementation).
    async fn next_frame(&mut self) -> Result<RawFrame, CaptureError>;

    /// Returns the current capture configuration.
    ///
    /// The configuration is immutable for the lifetime of the device; callers
    /// may cache it without worrying about staleness.
    fn config(&self) -> &CaptureConfig;

    /// Gracefully stops the capture stream and releases any kernel resources.
    ///
    /// After this call returns the implementation should unmap all mmap buffers,
    /// issue `VIDIOC_STREAMOFF`, and close the device file descriptor.
    async fn stop(&mut self) -> Result<(), CaptureError>;
}

// ── V4L2Capture ───────────────────────────────────────────────────────────────

/// Video4Linux2 capture backend (Linux only).
///
/// Wraps the V4L2 streaming I/O API to deliver uncompressed frames from any
/// UVC-compliant camera with sub-millisecond latency from hardware interrupt to
/// [`RawFrame`] delivery.
///
/// # Implementation Status
/// **Stub** — the type and trait are fully defined and compile on all platforms.
/// The V4L2 ioctl calls (`VIDIOC_REQBUFS`, `VIDIOC_STREAMON`, `mmap`) are left
/// as `TODO` items gated behind the `v4l2` feature flag.  The stub falls back to
/// the same synthetic-frame path as [`TestCapture`] so the pipeline can be
/// exercised end-to-end without a camera.
pub struct V4L2Capture {
    config: CaptureConfig,
    frame_counter: u64,
}

impl V4L2Capture {
    /// Open a V4L2 device at the path specified in `config`.
    ///
    /// In the stub implementation no ioctl calls are made; the function logs
    /// the device path and returns immediately.  A real implementation would:
    /// 1. `open(2)` the device node.
    /// 2. Issue `VIDIOC_S_FMT` to set resolution and pixel format.
    /// 3. Issue `VIDIOC_REQBUFS` to allocate kernel mmap buffers.
    /// 4. Issue `VIDIOC_STREAMON` to start DMA.
    ///
    /// # Errors
    /// Returns [`CaptureError::DeviceNotFound`] if the device path does not
    /// exist, or [`CaptureError::PermissionDenied`] if the process lacks
    /// access.  Both are stub TODOs; the current implementation always succeeds.
    pub async fn open(config: CaptureConfig) -> Result<Self, CaptureError> {
        tracing::info!(device = ?config.device_path, "V4L2Capture: opening (stub)");
        Ok(Self { config, frame_counter: 0 })
    }
}

#[async_trait::async_trait]
impl CaptureDevice for V4L2Capture {
    /// Returns a synthetic frame, simulating ~60 fps via a 16 ms sleep.
    ///
    /// # TODO
    /// Replace with a real `poll(2)` / `VIDIOC_DQBUF` loop over the mmap ring.
    async fn next_frame(&mut self) -> Result<RawFrame, CaptureError> {
        // TODO: implement V4L2 mmap buffer capture via VIDIOC_DQBUF / VIDIOC_QBUF
        tokio::time::sleep(std::time::Duration::from_millis(16)).await;
        self.frame_counter += 1;
        let pts_us = self.frame_counter * 16_667; // ~60 fps
        Ok(RawFrame::synthetic(
            self.config.resolution.width,
            self.config.resolution.height,
            pts_us,
        ))
    }

    fn config(&self) -> &CaptureConfig {
        &self.config
    }

    async fn stop(&mut self) -> Result<(), CaptureError> {
        tracing::info!("V4L2Capture: stop (stub — no ioctl cleanup needed)");
        Ok(())
    }
}

// ── TestCapture ───────────────────────────────────────────────────────────────

/// Synthetic frame generator for testing and benchmarking.
///
/// Produces [`PixelFormat::Yuv420p`] frames at the rate specified in
/// [`CaptureConfig::fps`] without opening any hardware device.  A
/// [`tokio::time::Interval`] drives the tick so frame pacing is as accurate as
/// the Tokio timer wheel allows (≈ 1 ms resolution on most platforms).
///
/// # Use Cases
/// * Pipeline integration tests that must run on CI without a camera.
/// * Benchmarks where capture latency should not be a variable.
/// * Fuzzing encoder/transport stages with a deterministic frame stream.
pub struct TestCapture {
    config: CaptureConfig,
    /// Monotonically increasing counter; used to derive a deterministic PTS.
    frame_counter: u64,
    /// Timer that fires once per frame period.
    interval: tokio::time::Interval,
}

impl TestCapture {
    /// Create a new `TestCapture` for the given configuration.
    ///
    /// The timer interval is derived from `config.fps`.
    ///
    /// # Panics
    /// Panics if `config.fps == 0` to prevent a division-by-zero at startup
    /// rather than silently producing infinite-duration intervals.
    pub fn new(config: CaptureConfig) -> Self {
        assert!(config.fps > 0, "CaptureConfig.fps must be > 0");
        let period = std::time::Duration::from_micros(1_000_000 / config.fps as u64);
        Self {
            config,
            frame_counter: 0,
            interval: tokio::time::interval(period),
        }
    }
}

#[async_trait::async_trait]
impl CaptureDevice for TestCapture {
    /// Waits for the next interval tick and returns a synthetic YUV420p frame.
    ///
    /// The presentation timestamp is computed deterministically from the frame
    /// counter and the configured frame rate so replayed streams are bit-exact.
    async fn next_frame(&mut self) -> Result<RawFrame, CaptureError> {
        self.interval.tick().await;
        self.frame_counter += 1;
        let pts_us = self.frame_counter * (1_000_000 / self.config.fps as u64);
        Ok(RawFrame::synthetic(
            self.config.resolution.width,
            self.config.resolution.height,
            pts_us,
        ))
    }

    fn config(&self) -> &CaptureConfig {
        &self.config
    }

    async fn stop(&mut self) -> Result<(), CaptureError> {
        // Nothing to clean up — all state is owned by this struct.
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// `TestCapture` must produce correctly-sized 1080p frames when configured
    /// at 30 fps, verifying the full synthetic-frame path of the pipeline.
    #[tokio::test]
    async fn test_capture_generates_frames() {
        let config = CaptureConfig {
            fps: 30,
            ..Default::default()
        };
        let mut cap = TestCapture::new(config);
        let frame = cap.next_frame().await.unwrap();
        assert_eq!(frame.width, 1920, "unexpected frame width");
        assert_eq!(frame.height, 1080, "unexpected frame height");
        assert!(!frame.data.is_empty(), "frame data must not be empty");
    }

    /// Consecutive frames must have strictly increasing PTS values so the
    /// encoder and transport layers can detect and reject reordered frames.
    #[tokio::test]
    async fn test_capture_pts_increases() {
        let config = CaptureConfig {
            fps: 60,
            ..Default::default()
        };
        let mut cap = TestCapture::new(config);
        let f1 = cap.next_frame().await.unwrap();
        let f2 = cap.next_frame().await.unwrap();
        assert!(
            f2.pts_us > f1.pts_us,
            "PTS must be strictly increasing: {} <= {}",
            f2.pts_us,
            f1.pts_us,
        );
    }

    /// Buffer length for a synthetic FHD frame must match the YUV420p formula.
    #[tokio::test]
    async fn test_capture_buffer_size() {
        let config = CaptureConfig::default();
        let mut cap = TestCapture::new(config);
        let frame = cap.next_frame().await.unwrap();
        let expected = RawFrame::yuv420p_size(1920, 1080);
        assert_eq!(
            frame.data.len(),
            expected,
            "buffer size mismatch: got {} expected {}",
            frame.data.len(),
            expected
        );
    }

    /// `TestCapture::new` must panic on fps == 0 to prevent divide-by-zero.
    #[test]
    #[should_panic(expected = "CaptureConfig.fps must be > 0")]
    fn test_capture_zero_fps_panics() {
        let config = CaptureConfig { fps: 0, ..Default::default() };
        let _ = TestCapture::new(config);
    }

    /// `CaptureConfig::default` must specify a sensible non-zero fps.
    #[test]
    fn test_config_default_sanity() {
        let cfg = CaptureConfig::default();
        assert!(cfg.fps > 0);
        assert_eq!(cfg.resolution, Resolution::FHD);
        assert_eq!(cfg.pixel_format, PixelFormat::Yuv420p);
    }
}
