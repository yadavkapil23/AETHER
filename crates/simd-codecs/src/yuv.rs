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

                    // BT.601 studio-swing formula
                    // C = Y - 16
                    // R = clamp((298*C + 409*E + 128) >> 8, 0, 255)
                    // G = clamp((298*C - 100*D - 208*E + 128) >> 8, 0, 255)
                    // B = clamp((298*C + 516*D + 128) >> 8, 0, 255)
                    let c = y_val - 16;
                    let r = clamp_u8((298 * c + 409 * e + 128) >> 8);
                    let g = clamp_u8((298 * c - 100 * d - 208 * e + 128) >> 8);
                    let b = clamp_u8((298 * c + 516 * d + 128) >> 8);

                    let rgb_base = (py * width + px) * 3;
                    rgb[rgb_base] = r;
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
/// inner iteration, falling back to the scalar path for any trailing columns
/// when `width` is not a multiple of 16.
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

    // BT.601 coefficients scaled by 2^8 (fixed-point, matching scalar path).
    // We work in i16: intermediate values fit because Y∈[16,235], so C∈[0,219]
    // and 298*219 = 65262 < i16::MAX is NOT true — we need i32 for the multiply.
    // Strategy: unpack u8→i16, apply coefficients one channel at a time keeping
    // values in i32-equivalent range by using saturating arithmetic and careful
    // ordering, then pack back to u8.
    //
    // AVX2 has _mm256_mullo_epi16 (low 16 bits of i16×i16) which can overflow
    // for coefficients > 127.  We therefore split the multiply:
    //   298 = 256 + 32 + 8 + 2  →  (val << 8) + (val << 5) + (val << 3) + (val << 1)
    // but that's error-prone.  Instead we use _mm256_madd_epi16 with a paired
    // constant approach or stay with the 16-bit multiply and handle the limited
    // dynamic range carefully.
    //
    // Practical approach used here:
    //   • Unpack Y bytes to i16 words (subtract 16 in i16).
    //   • Unpack U/V bytes to i16 words (subtract 128 in i16).
    //   • Scale C (=Y-16) by 298 using _mm256_mullo_epi16.  Max value of C is
    //     219, 298*219 = 65262 which overflows signed i16 (max 32767) — so we
    //     use a two-step multiply: 256*C via shift plus 42*C via mullo, then add.
    //   • The 409*E, 516*D, 100*D, 208*E terms all fit in i16 for their input
    //     ranges (E,D ∈ [-128,127]).
    //   • Add the bias (+128 before >> 8), shift right by 8 (divide by 256) with
    //     _mm256_srai_epi16, then pack to u8 with saturating _mm256_packus_epi16.

    // We process 16 pixels per iteration (two rows of 8 adjacent pixels sharing
    // one row of 8 UV pairs, giving 16 Y samples with 8 U and 8 V samples).

    // Precompute constants as i16 vectors.
    // SAFETY: _mm256_set1_epi16 is safe to call with any i16 value.
    let c16_16   = _mm256_set1_epi16(16_i16);   // Y offset
    let c16_128  = _mm256_set1_epi16(128_i16);  // UV offset / rounding bias
    let c16_409  = _mm256_set1_epi16(409_i16);  // V→R coefficient
    let c16_n100 = _mm256_set1_epi16(-100_i16); // U→G coefficient
    let c16_n208 = _mm256_set1_epi16(-208_i16); // V→G coefficient
    let c16_516  = _mm256_set1_epi16(516_i16);  // U→B coefficient
    // 298 = 256 + 42; split to avoid signed i16 overflow in mullo.
    let c16_42   = _mm256_set1_epi16(42_i16);
    let c16_min  = _mm256_set1_epi16(0_i16);
    let c16_max  = _mm256_set1_epi16(255_i16);

    // We'll write RGB interleaved.  AVX2 doesn't have a 3-channel scatter, so
    // we collect 8 R, 8 G, 8 B i16 lanes then extract to u8 and write via
    // a small scalar loop over the 8-pixel group.
    // (A full shuffle-based RGB pack would be faster but far more complex;
    //  this is already a meaningful speedup over pure scalar.)

    let uv_row_stride = width / 2;

    for row in 0..height {
        let uv_row = row / 2;
        let y_row_base = row * width;
        let uv_row_base = uv_row * uv_row_stride;
        let rgb_row_base = row * width * 3;

        // Number of full 16-pixel groups in this row.
        let chunks16 = width / 16;
        let remainder = width % 16;

        for chunk in 0..chunks16 {
            let px_base = chunk * 16;           // pixel offset within row
            let uv_base = chunk * 8;            // UV offset (2:1 horizontal subsample)

            // ── Load Y (16 bytes → 256-bit via two 128-bit loads) ─────────────
            // SAFETY: bounds checked by validate(); px_base+16 ≤ width ≤ len.
            let y_ptr = y_plane.as_ptr().add(y_row_base + px_base);
            // Load 16 u8 into lower 128 bits, zero-extend to 256-bit register
            // by zero-extending each byte to i16 word.
            let y_raw128 = _mm_loadu_si128(y_ptr as *const __m128i);
            // Unpack lower 8 bytes u8→i16
            let y_lo = _mm256_cvtepu8_epi16(y_raw128);
            // y_lo now has 16 × i16 values for the 16 Y samples.
            let c_lo = _mm256_sub_epi16(y_lo, c16_16); // C = Y - 16

            // ── Load U (8 bytes for 16 pixels) ────────────────────────────────
            // SAFETY: bounds checked; uv_base+8 ≤ width/2 ≤ len.
            let u_ptr = u_plane.as_ptr().add(uv_row_base + uv_base);
            let u_raw64 = _mm_loadl_epi64(u_ptr as *const __m128i);
            let u16 = _mm256_cvtepu8_epi16(u_raw64);   // 8 × i16 (only low 8 used)
            // Each U sample is shared by 2 adjacent pixels; duplicate: [u0 u0 u1 u1 ...]
            let u16_dup = _mm256_unpacklo_epi16(u16, u16); // interleave with itself
            // After unpacklo: [u0 u0 u1 u1 u2 u2 u3 u3 | u0 u0 u1 u1 u2 u2 u3 u3]
            // but we need [u0 u0 u1 u1 u2 u2 u3 u3 u4 u4 u5 u5 u6 u6 u7 u7].
            // _mm256_unpacklo_epi16 operates independently on the two 128-bit lanes,
            // so the high lane repeats the low 4 U values.  We need a permute.
            // Actually _mm256_cvtepu8_epi16 of 8 bytes gives [u0..u7, 0..0] in the
            // 256-bit register with u0..u7 in the low 128 bits and zeros in the high.
            // unpacklo_epi16(u16, u16) gives [u0,u0,u1,u1,u2,u2,u3,u3 | u0,u0,u1,u1,u2,u2,u3,u3].
            // We want [u0,u0,u1,u1,u2,u2,u3,u3 | u4,u4,u5,u5,u6,u6,u7,u7].
            // Fix: use unpackhi_epi16 for the upper half and permute2x128.
            let u16_hi_dup = _mm256_unpackhi_epi16(u16, u16);
            // u16_hi_dup low lane = [u4,u4,u5,u5,u6,u6,u7,u7], high lane = [0...]
            // Combine: low 128 from u16_dup, high 128 from u16_hi_dup.
            let d_full = _mm256_permute2x128_si256(u16_dup, u16_hi_dup, 0x20);
            // Now d_full = [u0,u0,u1,u1,...,u7,u7] (16 × i16)
            let d_full = _mm256_sub_epi16(d_full, c16_128); // D = U - 128

            // ── Load V (same approach as U) ───────────────────────────────────
            let v_ptr = v_plane.as_ptr().add(uv_row_base + uv_base);
            let v_raw64 = _mm_loadl_epi64(v_ptr as *const __m128i);
            let v16 = _mm256_cvtepu8_epi16(v_raw64);
            let v16_lo_dup = _mm256_unpacklo_epi16(v16, v16);
            let v16_hi_dup = _mm256_unpackhi_epi16(v16, v16);
            let e_full = _mm256_permute2x128_si256(v16_lo_dup, v16_hi_dup, 0x20);
            let e_full = _mm256_sub_epi16(e_full, c16_128); // E = V - 128

            // ── Apply BT.601 coefficients ─────────────────────────────────────
            // 298*C = 256*C + 42*C  (split to stay in i16 range for each term)
            let c256 = _mm256_slli_epi16(c_lo, 8);   // 256 * C  (may wrap if C > 127,
                                                       // but C = Y-16 ∈ [0,219] so
                                                       // 256*219 = 56064 < 32767 is FALSE
                                                       // → wraps for C > 127)
            // Since 256*C overflows i16 for C>127, we instead compute the full
            // channel value in two i16 halves and use saturating pack at the end.
            // Alternative: compute using i32 via _mm256_madd_epi16.
            // We use a simpler split: compute (298*C + bias + coeff) directly but
            // with i32 using _mm256_madd_epi16 pairing with 1.
            //
            // _mm256_madd_epi16(a, b): multiplies adjacent pairs of i16 and sums
            // to i32.  To use it for element-wise multiply: pair each element with 1.
            // [a0, 1, a1, 1, ...] × [298, 0, 298, 0, ...] → [298*a0, 298*a1, ...]
            // That requires reshuffling. Instead, use a simpler correct approach:
            //
            // Compute in i16 but cap intermediate at i16 boundaries via saturating add.
            // Use _mm256_mulhi_epi16 and _mm256_mullo_epi16:
            //   full product (32-bit) = hi<<16 | lo
            //   298 * c_lo: we need the low 16 bits of (298 * C) plus carry into the
            //   next 16 bits.  Since our final result is >>8, we effectively compute:
            //     result_before_shift = 298*C + coeff*D_or_E + 128
            //     final = result_before_shift >> 8   ∈ [0, 255] after clamp
            //
            // The most robust i16-only approach: observe that C ∈ [0, 219],
            // coeff_max * range = 516 * 255 = 131580, which overflows i16.
            // We MUST either use i32 intermediates or decompose the coefficient.
            //
            // Final approach: use _mm256_madd_epi16 for true 32-bit intermediates,
            // then shift and pack.  We process R, G, B channels separately,
            // computing for each channel the result as i32 via madd, shifting,
            // and saturating-packing pairs of i32 lanes to i16, then to u8.

            // ─ R = (298*C + 409*E + 128) >> 8 ─────────────────────────────
            //
            // Step 1: compute 298*C in 32-bit.
            // _mm256_madd_epi16 multiplies pairs [a0*b0 + a1*b1, a2*b2 + a3*b3, ...]
            // To do element-wise: interleave with zeros.
            let zero = _mm256_setzero_si256();
            // Interleave c_lo with zero: [c0, 0, c1, 0, c2, 0, c3, 0, ...]
            // → madd with [298, 0, 298, 0, ...] → [298*c0, 298*c1, ...]
            let c_lo_interleaved = _mm256_unpacklo_epi16(c_lo, zero);
            let c_hi_interleaved = _mm256_unpackhi_epi16(c_lo, zero);
            let coeff_298 = _mm256_set1_epi16(298_i16);
            // madd: [c0 * 298 + 0 * 0, c1 * 298 + 0 * 0, ...] = [298*c0, 298*c1, ...] as i32
            let term_298c_lo = _mm256_madd_epi16(c_lo_interleaved, _mm256_unpacklo_epi16(coeff_298, zero));
            let term_298c_hi = _mm256_madd_epi16(c_hi_interleaved, _mm256_unpackhi_epi16(coeff_298, zero));

            // 409*E in 32-bit
            let e_lo_interleaved = _mm256_unpacklo_epi16(e_full, zero);
            let e_hi_interleaved = _mm256_unpackhi_epi16(e_full, zero);
            let coeff_409 = _mm256_set1_epi16(409_i16);
            let term_409e_lo = _mm256_madd_epi16(e_lo_interleaved, _mm256_unpacklo_epi16(coeff_409, zero));
            let term_409e_hi = _mm256_madd_epi16(e_hi_interleaved, _mm256_unpackhi_epi16(coeff_409, zero));

            let bias = _mm256_set1_epi32(128_i32);
            let r32_lo = _mm256_add_epi32(_mm256_add_epi32(term_298c_lo, term_409e_lo), bias);
            let r32_hi = _mm256_add_epi32(_mm256_add_epi32(term_298c_hi, term_409e_hi), bias);
            // >> 8
            let r32_lo = _mm256_srai_epi32(r32_lo, 8);
            let r32_hi = _mm256_srai_epi32(r32_hi, 8);
            // Pack i32 → i16 with saturation (clamps to [-32768, 32767])
            let r16 = _mm256_packs_epi32(r32_lo, r32_hi);
            // After packs_epi32: lane ordering is [lo0..lo3, hi0..hi3 | lo4..lo7, hi4..hi7]
            // We need to fix the interleaving back to sequential order.
            // permute2x128 to bring lo-lane from both together and hi-lane together:
            let r16 = _mm256_permute4x64_epi64(r16, 0b_11_01_10_00); // [0,2,1,3] → sequential
            // Clamp to [0, 255] using i16 min/max
            let r16 = _mm256_max_epi16(r16, c16_min);
            let r16 = _mm256_min_epi16(r16, c16_max);

            // ─ G = (298*C - 100*D - 208*E + 128) >> 8 ─────────────────────
            let d_lo_interleaved = _mm256_unpacklo_epi16(d_full, zero);
            let d_hi_interleaved = _mm256_unpackhi_epi16(d_full, zero);
            // -100*D
            let neg100 = _mm256_set1_epi16(-100_i16);
            let term_n100d_lo = _mm256_madd_epi16(d_lo_interleaved, _mm256_unpacklo_epi16(neg100, zero));
            let term_n100d_hi = _mm256_madd_epi16(d_hi_interleaved, _mm256_unpackhi_epi16(neg100, zero));
            // -208*E
            let neg208 = _mm256_set1_epi16(-208_i16);
            let term_n208e_lo = _mm256_madd_epi16(e_lo_interleaved, _mm256_unpacklo_epi16(neg208, zero));
            let term_n208e_hi = _mm256_madd_epi16(e_hi_interleaved, _mm256_unpackhi_epi16(neg208, zero));

            let g32_lo = _mm256_add_epi32(
                _mm256_add_epi32(term_298c_lo, term_n100d_lo),
                _mm256_add_epi32(term_n208e_lo, bias),
            );
            let g32_hi = _mm256_add_epi32(
                _mm256_add_epi32(term_298c_hi, term_n100d_hi),
                _mm256_add_epi32(term_n208e_hi, bias),
            );
            let g32_lo = _mm256_srai_epi32(g32_lo, 8);
            let g32_hi = _mm256_srai_epi32(g32_hi, 8);
            let g16 = _mm256_packs_epi32(g32_lo, g32_hi);
            let g16 = _mm256_permute4x64_epi64(g16, 0b_11_01_10_00);
            let g16 = _mm256_max_epi16(g16, c16_min);
            let g16 = _mm256_min_epi16(g16, c16_max);

            // ─ B = (298*C + 516*D + 128) >> 8 ─────────────────────────────
            let coeff_516 = _mm256_set1_epi16(516_i16);
            let term_516d_lo = _mm256_madd_epi16(d_lo_interleaved, _mm256_unpacklo_epi16(coeff_516, zero));
            let term_516d_hi = _mm256_madd_epi16(d_hi_interleaved, _mm256_unpackhi_epi16(coeff_516, zero));
            let b32_lo = _mm256_add_epi32(_mm256_add_epi32(term_298c_lo, term_516d_lo), bias);
            let b32_hi = _mm256_add_epi32(_mm256_add_epi32(term_298c_hi, term_516d_hi), bias);
            let b32_lo = _mm256_srai_epi32(b32_lo, 8);
            let b32_hi = _mm256_srai_epi32(b32_hi, 8);
            let b16 = _mm256_packs_epi32(b32_lo, b32_hi);
            let b16 = _mm256_permute4x64_epi64(b16, 0b_11_01_10_00);
            let b16 = _mm256_max_epi16(b16, c16_min);
            let b16 = _mm256_min_epi16(b16, c16_max);

            // ── Pack 16×i16 R, G, B → u8 and write interleaved RGB ────────────
            // Pack r16, g16, b16 to u8 using packus.
            // _mm256_packus_epi16: packs two i16 vectors to u8 with unsigned saturation.
            // Result order after pack across 128-bit lanes needs a permute fix.
            // We'll extract each channel to a 128-bit register (16 bytes → 16 u8)
            // and then write interleaved in a scalar loop (simple, correct, small overhead).

            let mut r_buf = [0u8; 32];
            let mut g_buf = [0u8; 32];
            let mut b_buf = [0u8; 32];

            // Pack each channel pair with zeros to get the 16 u8 values.
            let r_u8 = _mm256_packus_epi16(r16, zero);
            let g_u8 = _mm256_packus_epi16(g16, zero);
            let b_u8 = _mm256_packus_epi16(b16, zero);
            // packus_epi16 with zero fills the high 8 bytes of each 128-bit lane with 0.
            // The 16 valid u8 values are at [0..8] and [16..24] of the 32-byte register.
            // Fix with permute so valid bytes are contiguous.
            let r_u8 = _mm256_permute4x64_epi64(r_u8, 0b_11_01_10_00);
            let g_u8 = _mm256_permute4x64_epi64(g_u8, 0b_11_01_10_00);
            let b_u8 = _mm256_permute4x64_epi64(b_u8, 0b_11_01_10_00);

            // SAFETY: r_buf/g_buf/b_buf are 32-byte aligned stack arrays; storeu is safe.
            _mm256_storeu_si256(r_buf.as_mut_ptr() as *mut __m256i, r_u8);
            _mm256_storeu_si256(g_buf.as_mut_ptr() as *mut __m256i, g_u8);
            _mm256_storeu_si256(b_buf.as_mut_ptr() as *mut __m256i, b_u8);

            // Write 16 interleaved RGB triplets.
            let rgb_base = rgb_row_base + px_base * 3;
            for i in 0..16usize {
                rgb[rgb_base + i * 3]     = r_buf[i];
                rgb[rgb_base + i * 3 + 1] = g_buf[i];
                rgb[rgb_base + i * 3 + 2] = b_buf[i];
            }
        }

        // ── Scalar tail for remaining pixels (width % 16 != 0) ────────────────
        if remainder > 0 {
            let px_start = chunks16 * 16;
            for px in px_start..width {
                let uv_col = px / 2;
                let uv_idx = uv_row * uv_row_stride + uv_col;
                let y_val = y_plane[y_row_base + px] as i32;
                let u = u_plane[uv_idx] as i32;
                let v = v_plane[uv_idx] as i32;
                let c = y_val - 16;
                let d = u - 128;
                let e = v - 128;
                let r = clamp_u8((298 * c + 409 * e + 128) >> 8);
                let g = clamp_u8((298 * c - 100 * d - 208 * e + 128) >> 8);
                let b = clamp_u8((298 * c + 516 * d + 128) >> 8);
                let base = rgb_row_base + px * 3;
                rgb[base]     = r;
                rgb[base + 1] = g;
                rgb[base + 2] = b;
            }
        }

        // Suppress unused-variable warnings for constants not used in all paths.
        let _ = (c16_42, c256);
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
