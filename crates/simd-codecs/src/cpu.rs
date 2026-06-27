//! `cpu` — runtime CPU-feature detection.
//!
//! [`CpuFeatures`] is designed to be called **once** at process startup and
//! then passed (cheaply, it is `Copy`) to any codec that needs to pick an
//! optimised code path.  Doing detection once avoids the overhead of
//! `is_x86_feature_detected!` being inlined at every call-site.

use std::fmt;

/// A snapshot of the SIMD extensions available on the current CPU core.
///
/// Use [`CpuFeatures::detect`] to populate this struct at startup, then
/// consult its fields to choose the right codec implementation path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuFeatures {
    /// Advanced Vector Extensions 2 — 256-bit integer SIMD (Intel Haswell+).
    pub avx2: bool,
    /// Streaming SIMD Extensions 4.2 — adds `PCMPGTQ`, `CRC32`, etc.
    pub sse42: bool,
    /// ARM NEON — 128-bit SIMD present on all AArch64 targets.
    pub neon: bool,
}

impl CpuFeatures {
    /// Detects CPU features at runtime.
    ///
    /// On x86/x86_64 this uses `cpuid` via [`std::is_x86_feature_detected!`].
    /// On AArch64 NEON is always available (mandatory in the ISA), so `neon`
    /// is set by compile-time `cfg`.  On other architectures all fields are
    /// `false`.
    ///
    /// # Example
    /// ```rust
    /// use simd_codecs::CpuFeatures;
    /// let f = CpuFeatures::detect();
    /// if f.avx2 { println!("AVX2 available — will use wide SIMD paths"); }
    /// ```
    pub fn detect() -> Self {
        // On non-x86 targets the macros below don't exist; guard them.
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        let (avx2, sse42) = (
            std::is_x86_feature_detected!("avx2"),
            std::is_x86_feature_detected!("sse4.2"),
        );
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
        let (avx2, sse42) = (false, false);

        Self {
            avx2,
            sse42,
            // NEON is mandatory on AArch64; detect via compile-time cfg so we
            // don't need a runtime probe.
            neon: cfg!(target_arch = "aarch64"),
        }
    }
}

impl fmt::Display for CpuFeatures {
    /// Formats as `CpuFeatures { avx2: true, sse4.2: false, neon: false }`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CpuFeatures {{ avx2: {}, sse4.2: {}, neon: {} }}",
            self.avx2, self.sse42, self.neon
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_does_not_panic() {
        let _ = CpuFeatures::detect();
    }

    #[test]
    fn display_contains_all_fields() {
        let s = CpuFeatures { avx2: true, sse42: false, neon: false }.to_string();
        assert!(s.contains("avx2: true"));
        assert!(s.contains("sse4.2: false"));
        assert!(s.contains("neon: false"));
    }
}
