//! `error` — unified error type for all codec operations.
//!
//! A single [`CodecError`] enum keeps callers from having to import multiple
//! error types and allows `?`-propagation across the whole crate.

/// All errors that can be returned by `simd-codecs` operations.
///
/// Implements [`std::error::Error`] via [`thiserror::Error`] so callers can
/// treat it as a boxed trait-object or integrate it into their own error
/// hierarchies with `#[from]`.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The caller supplied a buffer whose length does not match the size
    /// implied by the given frame dimensions.
    #[error("invalid buffer size: expected {expected}, got {got}")]
    InvalidBufferSize { expected: usize, got: usize },

    /// YUV 4:2:0 requires that both `width` and `height` are even so that each
    /// luma 2×2 block maps to exactly one chroma sample.
    #[error("unsupported dimension: width or height must be even for YUV420")]
    OddDimension,
}
