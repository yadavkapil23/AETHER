//! `yuv` — YUV 4:2:0 → RGB 24-bit colour-space conversion.
//!
//! Three entry-points are provided:
//!
//! | Function | Description |
//! |---|---|
//! | [`yuv420_to_rgb24_scalar`] | Pure-Rust BT.601 reference implementation |
//! | [`yuv420_to_rgb24_avx2`]  | AVX2-accelerated path (x86_64 only, unsafe) |
//! | [`yuv420_to_rgb24`]       | Runtime-dispatching wrapper — **prefer this** |
//!
//! All functions operate on planar YUV 4:2:0 (`YUV420p` / `I420`) laid out as
//! `[Y-plane][U-plane][V-plane]` with `width × height`, `(w/2)×(h/2)`,
//! `(w/2)×(h/2)` samples respectively.  The output is tightly-packed `RGB`
//! with 3 bytes per pixel in row-major order.

use crate::CodecError;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Validate buffer sizes and require even dimensions.
#[inline(always)]
fn validate(
    yuv_len: usize,
    rgb_len: usize,
    width: usize,
    height: usize,
) -> Result<(), CodecError> {
    if width % 2 != 0 || height % 2 != 0 {
        return Err(CodecError::OddDimension);
    }
    let expected_yuv = width * height * 3 / 2;
    if yuv_len < expected_yuv {
        return Err(CodecError::InvalidBufferSize {
            expected: expected_yuv,
            got: yuv_len,
        });
    }
    let expected_rgb = width * height * 3;
    if rgb_len < expected_rgb {
        return Err(CodecError::InvalidBufferSize {
            expected: expected_rgb,
            got: rgb_len,
        });
    }
    Ok(())
}

/// Clamp an `i32` to `[0, 255]` and return it as `u8`.
#[inline(always)]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

// ─── Scalar ───────────────────────────────────────────────────────────────────

/// Convert a planar YUV 4:2:0 frame to packed RGB24 using the BT.601 matrix.
///
/// This is the portable scalar reference implementation.  It is correct on all
/// targets and is used as the fallback when SIMD extensions are not available.
///
/// # Arguments
/// * `yuv`    — planar `[Y][U][V]` source buffer (`width*height*3/2` bytes)
/// * `rgb`    — packed `RGB` destination buffer (`width*height*3` bytes)
/// * `width`  — frame width in pixels (**must be even**)
/// * `height` — frame height in pixels (**must be even**)
///
/// # Errors
/// Returns [`CodecError::OddDimension`] if width or height is odd, or
/// [`CodecError::InvalidBufferSize`] if either buffer is too small.
pub fn yuv420_to_rgb24_scalar(
    yuv: &[u8],
    rgb: &mut [u8],
    width: usize,
    height: usize,
) -> Result<(), CodecError> {
    validate(yuv.len(), rgb.len(), width, height)?;

    let y_plane = &yuv[..width * height];
    let u_plane = &yuv[width * height..width * height + (width / 2) * (height / 2)];
    let v_plane = &yuv[width * height + (width / 2) * (height / 2)..];

    // Process the frame in 2×2 luma blocks, each sharing one (U, V) sample.
    for block_row in 0..height / 2 {
        for block_col in 0..width / 2 {
            let uv_idx = block_row * (width / 2) + block_col;
            let u = u_plane[uv_idx] as i32;
            let v = v_plane[uv_idx] as i32;

            let d = u - 128; // Cb offset
            let e = v - 128; // Cr offset

            // Process the 2×2 block of Y samples that share this (U,V) pair.
            for dy in 0..2usize {
                for dx in 0..2usize {
                    let py = block_row * 2 + dy;
                    let px = block_col * 2 + dx;
                    let y_val = y_plane[py * width + px] as i32;

                    // BT.601 studio-swing:
                    //   C = Y - 16
                    //   R = clamp((298·C + 409·E + 128) >> 8, 0, 255)
                    //   G = clamp((298·C − 100·D − 208·E + 128) >> 8, 0, 255)
                    //   B = clamp((298·C + 516·D + 128) >> 8, 0, 255)
                    let c = y_val - 16;
                    let r = clamp_u8((298 * c + 409 * e + 128) >> 8);
                    let g = clamp_u8((298 * c - 100 * d - 208 * e + 128) >> 8);
                    let b = clamp_u8((298 * c + 516 * d + 128) >> 8);

                    let rgb_base = (py * width + px) * 3;
                    rgb[rgb_base]     = r;
                    rgb[rgb_base + 1] = g;
                    rgb[rgb_base + 2] = b;
                }
            }
        }
    }

    Ok(())
}

// ─── AVX2 (x86_64 only) ───────────────────────────────────────────────────────

/// AVX2-accelerated YUV420p → RGB24 conversion using BT.601 coefficients.
///
/// Processes 16 Y-samples (two AVX2 lanes worth of 16-bit arithmetic) per
/// inner iteration.  Any trailing columns where `width % 16 != 0` are handled
/// by falling back to the scalar path.
///
/// # Safety
/// The caller **must** guarantee that the `avx2` CPU feature is available.
/// Use [`crate::CpuFeatures::detect`] or `std::is_x86_feature_detected!("avx2")`
/// before calling this function.  Calling without AVX2 present will trigger an
/// illegal-instruction fault.
///
/// # Errors
/// Same error conditions as [`yuv420_to_rgb24_scalar`].
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn yuv420_to_rgb24_avx2(
    yuv: &[u8],
    rgb: &mut [u8],
    width: usize,
    height: usize,
) -> Result<(), CodecError> {
    // SAFETY: target_feature avx2 is verified by caller via CpuFeatures::detect()
    // or std::is_x86_feature_detected!("avx2") before this function is invoked.
    use std::arch::x86_64::*;

    validate(yuv.len(), rgb.len(), width, height)?;

    let y_plane = &yuv[..width * height];
    let u_plane = &yuv[width * height..width * height + (width / 2) * (height / 2)];
    let v_plane = &yuv[width * height + (width / 2) * (height / 2)..];

    // ── Strategy ────────────────────────────────────────────────────────────
    //
    // BT.601 requires coefficients up to 516, and input values up to 219 (C)
    // or 127 (D, E).  Intermediate products reach 516*127 = 65532 which exceeds
    // i16::MAX (32767) — so we cannot use _mm256_mullo_epi16 directly for all
    // terms.
    //
    // We use _mm256_madd_epi16 (multiply-add → i32) instead:
    //   • Interleave each i16 vector with zeros to get alternating [val, 0, ...].
    //   • Create matching coefficient vector [coeff, 0, coeff, 0, ...].
    //   • madd produces element-wise 32-bit products without overflow.
    //
    // Processing 16 pixels/iteration:
    //   • Load 16 Y bytes, zero-extend to 16×i16.
    //   • Load 8 U/8 V bytes, zero-extend to 8×i16, then duplicate each entry
    //     to account for horizontal chroma subsampling (each UV pair covers 2 Ys).
    //   • Compute R, G, B channels independently in 32-bit, shift >> 8, pack back
    //     to i16 with saturation, clamp to [0,255], pack to u8.
    //   • Write 16 interleaved RGB triplets via a small scalar scatter loop.

    let uv_row_stride = width / 2;

    // ── Per-pixel scalar helper (re-used for tail) ────────────────────────────
    #[inline(always)]
    unsafe fn scalar_pixel(
        rgb: &mut [u8],
        base: usize,
        y_val: i32,
        u_val: i32,
        v_val: i32,
    ) {
        let c = y_val - 16;
        let d = u_val - 128;
        let e = v_val - 128;
        rgb[base]     = clamp_u8((298 * c + 409 * e + 128) >> 8);
        rgb[base + 1] = clamp_u8((298 * c - 100 * d - 208 * e + 128) >> 8);
        rgb[base + 2] = clamp_u8((298 * c + 516 * d + 128) >> 8);
    }

    for row in 0..height {
        let uv_row      = row / 2;
        let y_row_base  = row * width;
        let uv_row_base = uv_row * uv_row_stride;
        let rgb_row_base = row * width * 3;

        let chunks16 = width / 16;
        let remainder = width % 16;

        for chunk in 0..chunks16 {
            let px_base = chunk * 16; // first pixel of this 16-pixel group
            let uv_base = chunk * 8;  // first U/V sample (8 UV pairs = 16 Ys)

            // ── Load and zero-extend Y (16 u8 → 16 i16) ──────────────────────
            // SAFETY: px_base + 16 ≤ width (chunk < chunks16) and
            //         y_row_base + width ≤ y_plane.len() (validated above).
            let y_ptr   = y_plane.as_ptr().add(y_row_base + px_base);
            let y128    = _mm_loadu_si128(y_ptr as *const __m128i);
            // _mm256_cvtepu8_epi16: zero-extend 16 u8 → 16 i16
            let y16     = _mm256_cvtepu8_epi16(y128);
            // C = Y - 16
            let c16_16  = _mm256_set1_epi16(16_i16);
            let c_vec   = _mm256_sub_epi16(y16, c16_16);

            // ── Load U (8 u8) and duplicate each sample for 2 adjacent pixels ──
            // SAFETY: uv_base + 8 ≤ width/2 (chunk < chunks16, so uv_base+8 =
            //         (chunk+1)*8 ≤ chunks16*8 = width/16*8 ≤ width/2).
            let u_ptr   = u_plane.as_ptr().add(uv_row_base + uv_base);
            let u64_raw = _mm_loadl_epi64(u_ptr as *const __m128i);
            // zero-extend 8 u8 → 8 i16 (in low 128-bit lane of 256-bit reg)
            let u8_as16 = _mm256_cvtepu8_epi16(u64_raw);
            // Duplicate: [u0,u1,u2,u3, u4,u5,u6,u7, 0,0,...] ← cvtepu8_epi16 output
            // We need [u0,u0, u1,u1, u2,u2, u3,u3, u4,u4, u5,u5, u6,u6, u7,u7].
            // Strategy: unpacklo gives [u0,u0,u1,u1,u2,u2,u3,u3] in low lane (repeated in high).
            //           unpackhi gives [u4,u4,u5,u5,u6,u6,u7,u7] in low lane (zeros in high).
            //           permute2x128(lo_dup, hi_dup, 0x20) → [lo_low | hi_low] = correct.
            let u_lo_dup = _mm256_unpacklo_epi16(u8_as16, u8_as16); // [u0,u0..u3,u3 | u0,u0..u3,u3]
            let u_hi_dup = _mm256_unpackhi_epi16(u8_as16, u8_as16); // [u4,u4..u7,u7 | 0...]
            let d_vec16  = _mm256_permute2x128_si256(u_lo_dup, u_hi_dup, 0x20);
            // D = U - 128
            let c16_128  = _mm256_set1_epi16(128_i16);
            let d_vec    = _mm256_sub_epi16(d_vec16, c16_128);

            // ── Load V (same structure as U) ──────────────────────────────────
            let v_ptr    = v_plane.as_ptr().add(uv_row_base + uv_base);
            let v64_raw  = _mm_loadl_epi64(v_ptr as *const __m128i);
            let v8_as16  = _mm256_cvtepu8_epi16(v64_raw);
            let v_lo_dup = _mm256_unpacklo_epi16(v8_as16, v8_as16);
            let v_hi_dup = _mm256_unpackhi_epi16(v8_as16, v8_as16);
            let e_vec16  = _mm256_permute2x128_si256(v_lo_dup, v_hi_dup, 0x20);
            let e_vec    = _mm256_sub_epi16(e_vec16, c16_128);

            // ── madd helper: element-wise i16×i16 → i32 via paired-zero trick ──
            // Interleave a vector with zeros → [a0, 0, a1, 0, ...] (16 i16)
            // madd with [k, 0, k, 0, ...] → [k*a0 + 0*0, k*a1 + 0*0, ...] as 8 i32.
            // We call this twice per vector (lo/hi halves) to get all 16 results.
            // SAFETY: all inputs are properly initialised i16 vectors.
            let zero256  = _mm256_setzero_si256();

            // Compute 298*C in 32-bit (C ∈ [0,219], 298*219=65262 overflows i16)
            let k298     = _mm256_set1_epi16(298_i16);
            let c_lo32   = _mm256_madd_epi16(
                _mm256_unpacklo_epi16(c_vec, zero256),
                _mm256_unpacklo_epi16(k298,  zero256),
            );
            let c_hi32   = _mm256_madd_epi16(
                _mm256_unpackhi_epi16(c_vec, zero256),
                _mm256_unpackhi_epi16(k298,  zero256),
            );

            // 409*E  (E ∈ [-128,127], 409*128=52352 > i16::MAX — use i32)
            let k409     = _mm256_set1_epi16(409_i16);
            let e409_lo  = _mm256_madd_epi16(
                _mm256_unpacklo_epi16(e_vec, zero256),
                _mm256_unpacklo_epi16(k409,  zero256),
            );
            let e409_hi  = _mm256_madd_epi16(
                _mm256_unpackhi_epi16(e_vec, zero256),
                _mm256_unpackhi_epi16(k409,  zero256),
            );

            // -100*D  (safe in i16 for D ∈ [-128,127]: 100*128=12800 < 32767)
            // We still use madd for consistency and to avoid dealing with the
            // sign extension of the negative coefficient in unpack.
            let kn100    = _mm256_set1_epi16(-100_i16);
            let dn100_lo = _mm256_madd_epi16(
                _mm256_unpacklo_epi16(d_vec, zero256),
                _mm256_unpacklo_epi16(kn100, zero256),
            );
            let dn100_hi = _mm256_madd_epi16(
                _mm256_unpackhi_epi16(d_vec, zero256),
                _mm256_unpackhi_epi16(kn100, zero256),
            );

            // -208*E  (208*128=26624 < 32767, but we stay in 32-bit for safety)
            let kn208    = _mm256_set1_epi16(-208_i16);
            let en208_lo = _mm256_madd_epi16(
                _mm256_unpacklo_epi16(e_vec, zero256),
                _mm256_unpacklo_epi16(kn208, zero256),
            );
            let en208_hi = _mm256_madd_epi16(
                _mm256_unpackhi_epi16(e_vec, zero256),
                _mm256_unpackhi_epi16(kn208, zero256),
            );

            // 516*D  (516*128=66048 > i16::MAX — must use 32-bit)
            let k516     = _mm256_set1_epi16(516_i16);
            let d516_lo  = _mm256_madd_epi16(
                _mm256_unpacklo_epi16(d_vec, zero256),
                _mm256_unpacklo_epi16(k516,  zero256),
            );
            let d516_hi  = _mm256_madd_epi16(
                _mm256_unpackhi_epi16(d_vec, zero256),
                _mm256_unpackhi_epi16(k516,  zero256),
            );

            let bias32   = _mm256_set1_epi32(128_i32);

            // ── R = (298·C + 409·E + 128) >> 8 ──────────────────────────────
            let r_lo = _mm256_srai_epi32(
                _mm256_add_epi32(_mm256_add_epi32(c_lo32, e409_lo), bias32), 8);
            let r_hi = _mm256_srai_epi32(
                _mm256_add_epi32(_mm256_add_epi32(c_hi32, e409_hi), bias32), 8);

            // ── G = (298·C − 100·D − 208·E + 128) >> 8 ──────────────────────
            let g_lo = _mm256_srai_epi32(
                _mm256_add_epi32(
                    _mm256_add_epi32(c_lo32, bias32),
                    _mm256_add_epi32(dn100_lo, en208_lo),
                ), 8);
            let g_hi = _mm256_srai_epi32(
                _mm256_add_epi32(
                    _mm256_add_epi32(c_hi32, bias32),
                    _mm256_add_epi32(dn100_hi, en208_hi),
                ), 8);

            // ── B = (298·C + 516·D + 128) >> 8 ──────────────────────────────
            let b_lo = _mm256_srai_epi32(
                _mm256_add_epi32(_mm256_add_epi32(c_lo32, d516_lo), bias32), 8);
            let b_hi = _mm256_srai_epi32(
                _mm256_add_epi32(_mm256_add_epi32(c_hi32, d516_hi), bias32), 8);

            // ── Pack i32 → i16 (saturating), fix lane order, clamp [0,255] ───
            // _mm256_packs_epi32 saturates i32→i16 and interleaves lanes:
            //   result = [lo0..lo3, hi0..hi3 | lo4..lo7, hi4..hi7]
            // _mm256_permute4x64_epi64 with imm8=0b11_01_10_00 reorders 64-bit
            // chunks to restore sequential order: [0,2,1,3]→[0,1,2,3].
            let clamp_lo = _mm256_set1_epi16(0_i16);
            let clamp_hi = _mm256_set1_epi16(255_i16);
            // imm8 = 0b_11_01_10_00: reorders 64-bit chunks [0,2,1,3]→[0,1,2,3]
            // to fix the interleaving artefact from _mm256_packs_epi32.
            // Must be a literal constant — _mm256_permute4x64_epi64 takes an imm8.

            let r16 = {
                let p = _mm256_packs_epi32(r_lo, r_hi);
                let p = _mm256_permute4x64_epi64(p, 0b_11_01_10_00_i32);
                let p = _mm256_max_epi16(p, clamp_lo);
                _mm256_min_epi16(p, clamp_hi)
            };
            let g16 = {
                let p = _mm256_packs_epi32(g_lo, g_hi);
                let p = _mm256_permute4x64_epi64(p, 0b_11_01_10_00_i32);
                let p = _mm256_max_epi16(p, clamp_lo);
                _mm256_min_epi16(p, clamp_hi)
            };
            let b16 = {
                let p = _mm256_packs_epi32(b_lo, b_hi);
                let p = _mm256_permute4x64_epi64(p, 0b_11_01_10_00_i32);
                let p = _mm256_max_epi16(p, clamp_lo);
                _mm256_min_epi16(p, clamp_hi)
            };

            // ── Pack i16 → u8 (unsigned saturating), contiguify, store ───────
            // _mm256_packus_epi16(v, zero): packs [v0..v15, 0..0] → the 16 valid
            // u8 values land at bytes [0..8] and [16..24] of the 256-bit result
            // (because packus works per 128-bit lane).  We permute to [0..16].
            let r_u8 = _mm256_permute4x64_epi64(
                _mm256_packus_epi16(r16, zero256), 0b_11_01_10_00_i32);
            let g_u8 = _mm256_permute4x64_epi64(
                _mm256_packus_epi16(g16, zero256), 0b_11_01_10_00_i32);
            let b_u8 = _mm256_permute4x64_epi64(
                _mm256_packus_epi16(b16, zero256), 0b_11_01_10_00_i32);

            // SAFETY: r/g/b_buf are 32-byte stack arrays ≥ alignment of __m256i
            //         (storeu does not require alignment); accesses are in-bounds.
            let mut r_buf = [0u8; 32];
            let mut g_buf = [0u8; 32];
            let mut b_buf = [0u8; 32];
            _mm256_storeu_si256(r_buf.as_mut_ptr() as *mut __m256i, r_u8);
            _mm256_storeu_si256(g_buf.as_mut_ptr() as *mut __m256i, g_u8);
            _mm256_storeu_si256(b_buf.as_mut_ptr() as *mut __m256i, b_u8);

            // Write 16 interleaved RGB triplets into the output buffer.
            // SAFETY: rgb_row_base + (px_base+16)*3 ≤ rgb.len() (validated).
            let rgb_base = rgb_row_base + px_base * 3;
            for i in 0..16usize {
                rgb[rgb_base + i * 3]     = r_buf[i];
                rgb[rgb_base + i * 3 + 1] = g_buf[i];
                rgb[rgb_base + i * 3 + 2] = b_buf[i];
            }
        }

        // ── Scalar tail: remaining pixels when width % 16 != 0 ───────────────
        if remainder > 0 {
            let px_start = chunks16 * 16;
            for px in px_start..width {
                let uv_col = px / 2;
                let uv_idx = uv_row * uv_row_stride + uv_col;
                // SAFETY: indices checked by validate().
                scalar_pixel(
                    rgb,
                    rgb_row_base + px * 3,
                    y_plane[y_row_base + px] as i32,
                    u_plane[uv_idx] as i32,
                    v_plane[uv_idx] as i32,
                );
            }
        }
    }

    Ok(())
}

// ─── Runtime-dispatching public API ───────────────────────────────────────────

/// Convert a planar YUV 4:2:0 frame to packed RGB24, automatically selecting
/// the fastest available implementation at runtime.
///
/// On x86_64 with AVX2 this calls the vectorised path; elsewhere it falls back
/// to the scalar reference implementation.  The hot dispatch check is a single
/// `cpuid`-backed boolean (evaluated once by the CPU, then branch-predicted).
///
/// # Arguments
/// Same as [`yuv420_to_rgb24_scalar`].
///
/// # Errors
/// Same as [`yuv420_to_rgb24_scalar`].
pub fn yuv420_to_rgb24(
    yuv: &[u8],
    rgb: &mut [u8],
    width: usize,
    height: usize,
) -> Result<(), CodecError> {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        // SAFETY: avx2 just confirmed present by is_x86_feature_detected!
        return unsafe { yuv420_to_rgb24_avx2(yuv, rgb, width, height) };
    }

    yuv420_to_rgb24_scalar(yuv, rgb, width, height)
}
