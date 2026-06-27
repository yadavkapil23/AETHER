//! `simd-codecs` — high-throughput video codec primitives for the AETHER pipeline.
//!
//! This crate exists to centralise all SIMD-accelerated codec work (colour-space
//! conversion, bilinear scaling) behind safe, ergonomic Rust APIs.  Every public
//! function performs runtime CPU-feature detection and automatically selects the
//! fastest available implementation path (AVX2 → SSE4.2 → scalar).
//!
//! # Quick-start
//! ```rust,no_run
//! use simd_codecs::{CpuFeatures, yuv420_to_rgb24, bilinear_scale};
//!
//! let features = CpuFeatures::detect();
//! println!("Running with: {features}");
//!
//! let width = 1920usize;
//! let height = 1080usize;
//! let yuv = vec![128u8; width * height * 3 / 2];
//! let mut rgb = vec![0u8; width * height * 3];
//! yuv420_to_rgb24(&yuv, &mut rgb, width, height).unwrap();
//! ```

mod cpu;
mod error;
mod scale;
mod yuv;

pub use cpu::CpuFeatures;
pub use error::CodecError;
pub use scale::bilinear_scale;
pub use yuv::{yuv420_to_rgb24, yuv420_to_rgb24_scalar};

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Build a YUV420p frame of `width × height` filled with constant luma `y`
    /// and neutral chroma (U=V=128).
    fn make_const_yuv(width: usize, height: usize, y_val: u8) -> Vec<u8> {
        let y_size = width * height;
        let uv_size = (width / 2) * (height / 2);
        let mut buf = vec![y_val; y_size + 2 * uv_size];
        // U and V planes are already 128 if y_val == 128, but set explicitly.
        for b in &mut buf[y_size..] {
            *b = 128;
        }
        buf
    }

    /// Build a realistic YUV420p frame with a Y-ramp (16..235) and neutral chroma.
    fn make_ramp_yuv(width: usize, height: usize) -> Vec<u8> {
        let y_size = width * height;
        let uv_size = (width / 2) * (height / 2);
        let mut buf = vec![128u8; y_size + 2 * uv_size];
        for (i, b) in buf[..y_size].iter_mut().enumerate() {
            *b = (16 + (i % 220)) as u8;
        }
        buf
    }

    // ── colour-accuracy tests ─────────────────────────────────────────────────

    /// BT.601 black: Y=16, U=128, V=128 → R=G=B=0
    #[test]
    fn scalar_black_maps_to_zero() {
        let width = 2;
        let height = 2;
        let yuv = make_const_yuv(width, height, 16);
        let mut rgb = vec![0u8; width * height * 3];
        yuv420_to_rgb24_scalar(&yuv, &mut rgb, width, height).unwrap();
        for (i, &v) in rgb.iter().enumerate() {
            assert_eq!(v, 0, "channel at index {i} should be 0 for black YUV");
        }
    }

    /// BT.601 white: Y=235, U=128, V=128 → R≈G≈B≈255 (allow ±3 rounding)
    #[test]
    fn scalar_white_maps_to_near_255() {
        let width = 2;
        let height = 2;
        let yuv = make_const_yuv(width, height, 235);
        let mut rgb = vec![0u8; width * height * 3];
        yuv420_to_rgb24_scalar(&yuv, &mut rgb, width, height).unwrap();
        for (i, &v) in rgb.iter().enumerate() {
            assert!(
                v >= 252,
                "channel at index {i} should be ≥252 for white YUV, got {v}"
            );
        }
    }

    // ── dispatch consistency ──────────────────────────────────────────────────

    /// The dispatch function must produce the same bytes as the scalar path on a
    /// 64×64 ramp frame regardless of which SIMD path is chosen at runtime.
    #[test]
    fn dispatch_matches_scalar_on_64x64() {
        let width = 64;
        let height = 64;
        let yuv = make_ramp_yuv(width, height);
        let mut rgb_scalar = vec![0u8; width * height * 3];
        let mut rgb_dispatch = vec![0u8; width * height * 3];

        yuv420_to_rgb24_scalar(&yuv, &mut rgb_scalar, width, height).unwrap();
        yuv420_to_rgb24(&yuv, &mut rgb_dispatch, width, height).unwrap();

        // AVX2 and scalar may differ by ±1 due to intermediate rounding; allow it.
        for (i, (&a, &b)) in rgb_scalar.iter().zip(rgb_dispatch.iter()).enumerate() {
            let diff = (a as i16 - b as i16).unsigned_abs();
            assert!(
                diff <= 1,
                "scalar[{i}]={a} vs dispatch[{i}]={b}: differ by more than 1"
            );
        }
    }

    // ── error handling ────────────────────────────────────────────────────────

    /// Odd width must be rejected.
    #[test]
    fn odd_width_returns_error() {
        let yuv = vec![0u8; 3 * 4 * 3 / 2]; // 3-wide (odd) × 4-tall, wrong size anyway
        let mut rgb = vec![0u8; 3 * 4 * 3];
        let result = yuv420_to_rgb24_scalar(&yuv, &mut rgb, 3, 4);
        assert!(
            matches!(result, Err(CodecError::OddDimension)),
            "expected OddDimension, got {result:?}"
        );
    }

    /// Passing a too-small rgb buffer must be rejected.
    #[test]
    fn small_rgb_buffer_returns_error() {
        let width = 4;
        let height = 4;
        let yuv = make_const_yuv(width, height, 16);
        let mut rgb = vec![0u8; 10]; // far too small
        let result = yuv420_to_rgb24_scalar(&yuv, &mut rgb, width, height);
        assert!(
            matches!(result, Err(CodecError::InvalidBufferSize { .. })),
            "expected InvalidBufferSize, got {result:?}"
        );
    }

    // ── bilinear scale ────────────────────────────────────────────────────────

    /// Scaling a 4×4 uniform image to 2×2 must preserve the pixel value exactly
    /// because every destination sample lands exactly on a source pixel.
    #[test]
    fn bilinear_uniform_source_preserved() {
        let channels = 3usize;
        let src = vec![200u8; 4 * 4 * channels];
        let mut dst = vec![0u8; 2 * 2 * channels];
        bilinear_scale(&src, &mut dst, 4, 4, 2, 2, channels).unwrap();
        for (i, &v) in dst.iter().enumerate() {
            assert_eq!(v, 200, "dst[{i}] should be 200 after scaling uniform source");
        }
    }

    /// Corner pixels must be reproduced exactly when scaling down (no interpolation
    /// at corner samples — they map directly onto source corners).
    #[test]
    fn bilinear_corners_preserved() {
        // 4×4 image where we can identify corners.
        let channels = 1usize;
        let mut src = vec![128u8; 4 * 4 * channels];
        // top-left = 10, top-right = 20, bottom-left = 30, bottom-right = 40
        src[0] = 10;
        src[3] = 20;
        src[12] = 30;
        src[15] = 40;

        let mut dst = vec![0u8; 2 * 2 * channels];
        bilinear_scale(&src, &mut dst, 4, 4, 2, 2, channels).unwrap();

        // With x_ratio=(4-1)/(2-1)=3, y_ratio=3:
        // dst(0,0) maps to src(0,0)=10; dst(0,1) maps to src(3,0)=20
        // dst(1,0) maps to src(0,3)=30; dst(1,1) maps to src(3,3)=40
        assert_eq!(dst[0], 10, "top-left corner mismatch");
        assert_eq!(dst[1], 20, "top-right corner mismatch");
        assert_eq!(dst[2], 30, "bottom-left corner mismatch");
        assert_eq!(dst[3], 40, "bottom-right corner mismatch");
    }
}
