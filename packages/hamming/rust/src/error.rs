use thiserror::Error;

#[derive(Error, Debug)]
pub enum HammingError {
    #[error("invalid fingerprint length: expected {expected}, got {actual}")]
    InvalidFingerprintLength { expected: usize, actual: usize },

    #[error("codebook address out of range: 0x{address:03X} (max 0xFFF)")]
    AddressOutOfRange { address: u32 },

    #[error("codebook entry not found at address 0x{address:03X}")]
    EntryNotFound { address: u32 },

    #[error("codebook version mismatch: expected {expected}, got {actual}")]
    VersionMismatch { expected: u32, actual: u32 },

    #[error("frame too large: {size} bytes exceeds maximum {max}")]
    FrameTooLarge { size: usize, max: usize },

    #[error("invalid frame magic: expected 0xA2B1, got 0x{0:04X}")]
    InvalidMagic(u16),

    #[error("HMAC verification failed")]
    HmacFailed,

    #[error("CRC32 checksum mismatch")]
    ChecksumMismatch,

    #[error("unsupported payload mode: {0}")]
    UnsupportedMode(u8),

    #[error("hamming distance {distance} exceeds error threshold {threshold}")]
    DistanceExceedsThreshold { distance: u32, threshold: u32 },

    #[error("xor stream delta failed: {0}")]
    XorDeltaFailed(String),

    #[error("compression error: {0}")]
    CompressionError(String),

    #[error("decompression error: {0}")]
    DecompressionError(String),

    #[error("serialization error: {0}")]
    SerializationError(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[cfg(feature = "grpc")]
    #[error("transport error: {0}")]
    Transport(String),
}

pub type Result<T> = std::result::Result<T, HammingError>;
