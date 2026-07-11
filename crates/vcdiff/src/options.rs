//! Public decoder resource limits

/// Controls decoder limits; an unlimited target can let untrusted deltas consume unbounded disk space
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// Maximum cumulative target size
    pub max_target_size: u64,
    /// Maximum decoded section bytes retained for one window
    pub max_window_memory: u64,
    /// Maximum LZMA2 dictionary size for each secondary stream
    pub max_secondary_dictionary_size: u64,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_target_size: u64::MAX,
            max_window_memory: 128 * 1024 * 1024,
            max_secondary_dictionary_size: 64 * 1024 * 1024,
        }
    }
}
