//! `scale` — bilinear image scaling.
//!
//! [`bilinear_scale`] resizes any planar or interleaved buffer with a configurable
//! number of channels.  It is used by the AETHER mixer for both luma-plane
//! downscaling and full-colour RGB rescaling before encoding.
//!
//! The current implementation is a high-quality scalar bilinear interpolation.
//! An AVX2 path that processes 8 destination pixels per iteration is planned
//! (see the TODO comment in [`bilinear_scale`]).

use crate::CodecError;

// ─── Scalar implementation ────────────────────────────────────────────────────

/// Bilinear-interpolate `src` (size `src_w × src_h × channels`) into `dst`
/// (size `dst_w × dst_h × channels`).
///
/// # Arguments
/// * `src`      — source pixel buffer in row-major order
/// * `dst`      — destination pixel buffer (must be pre-allocated)
/// * `src_w`    — source width in pixels
/// * `src_h`    — source height in pixels
/// * `dst_w`    — destination width in pixels
/// * `dst_h`    — destination height in pixels
/// * `channels` — number of interleaved channels (e.g. 1 for Y-plane, 3 for RGB)
///
/// # Errors
/// Returns [`CodecError::InvalidBufferSize`] if either buffer is too small for
/// the given dimensions.
pub fn bilinear_scale_scalar(
    src: &[u8],
    dst: &mut [u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    channels: usize,
) -> Result<(), CodecError> {
    let expected_src = src_w * src_h * channels;
    if src.len() < expected_src {
        return Err(CodecError::InvalidBufferSize {
            expected: expected_src,
            got: src.len(),
        });
    }
    let expected_dst = dst_w * dst_h * channels;
    if dst.len() < expected_dst {
        return Err(CodecError::InvalidBufferSize {
            expected: expected_dst,
            got: dst.len(),
        });
    }

    // Degenerate: 1×1 destination — copy top-left pixel.
    if dst_w == 1 && dst_h == 1 {
        for c in 0..channels {
            dst[c] = src[c];
        }
        return Ok(());
    }

    // x_ratio and y_ratio map a destination pixel coordinate to a floating-point
    // source coordinate.  We subtract 1 from both dimensions so that dst corners
    // map exactly onto src corners.
    let x_ratio = if dst_w > 1 {
        (src_w - 1) as f32 / (dst_w - 1) as f32
    } else {
        0.0f32
    };
    let y_ratio = if dst_h > 1 {
        (src_h - 1) as f32 / (dst_h - 1) as f32
    } else {
        0.0f32
    };

    for dy in 0..dst_h {
        let src_y_f = dy as f32 * y_ratio;
        let y0 = src_y_f as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let y_frac = src_y_f - y0 as f32; // fractional part ∈ [0, 1)

        for dx in 0..dst_w {
            let src_x_f = dx as f32 * x_ratio;
            let x0 = src_x_f as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let x_frac = src_x_f - x0 as f32; // fractional part ∈ [0, 1)

            // Bilinear weights.
            let w00 = (1.0 - x_frac) * (1.0 - y_frac);
            let w10 = x_frac * (1.0 - y_frac);
            let w01 = (1.0 - x_frac) * y_frac;
            let w11 = x_frac * y_frac;

            let dst_base = (dy * dst_w + dx) * channels;

            for c in 0..channels {
                let p00 = src[(y0 * src_w + x0) * channels + c] as f32;
                let p10 = src[(y0 * src_w + x1) * channels + c] as f32;
                let p01 = src[(y1 * src_w + x0) * channels + c] as f32;
                let p11 = src[(y1 * src_w + x1) * channels + c] as f32;
                let interpolated = w00 * p00 + w10 * p10 + w01 * p01 + w11 * p11;
                dst[dst_base + c] = interpolated.round() as u8;
            }
        }
    }

    Ok(())
}

// ─── Public API with future dispatch hook ────────────────────────────────────

/// Bilinear-scale `src` to `dst`, selecting the fastest available path.
///
/// Currently delegates to [`bilinear_scale_scalar`].  An AVX2 path is planned
/// for a future optimisation pass.
///
/// # Arguments
/// See [`bilinear_scale_scalar`] — arguments are identical.
///
/// # Errors
/// See [`bilinear_scale_scalar`].
pub fn bilinear_scale(
    src: &[u8],
    dst: &mut [u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    channels: usize,
) -> Result<(), CodecError> {
    // TODO(avx2): process 8 dst pixels per iteration using _mm256_* gather/blend
    bilinear_scale_scalar(src, dst, src_w, src_h, dst_w, dst_h, channels)
}
