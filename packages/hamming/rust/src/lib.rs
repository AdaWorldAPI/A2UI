//! # A2UI Hamming Protocol
//!
//! Bitpacked 10,000-bit Hamming fingerprint transport for A2UI.
//! Replaces JSON with a codebook-referenced binary protocol that fits
//! in UDP-sized frames (≤1,472 bytes).
//!
//! ## Architecture
//!
//! ```text
//! Agent (LLM) → Hamming Encoder → Frame Builder → UDP/gRPC → Frame Decoder → Renderer
//! ```
//!
//! ## Features
//!
//! - `simd` (default) — SIMD-accelerated Hamming distance
//! - `grpc` (default) — tonic gRPC transport
//! - `flight` — Arrow Flight bulk transport (ladybug-rs integration)
//! - `wasm` — Browser WASM bindings
//! - `cognitive` — ladybug-rs cognitive codebook
//! - `firefly` — firefly GEL execution graph

pub mod vector;
pub mod bitpack;
pub mod codebook;
pub mod frame;
pub mod xor_stream;
pub mod error;
pub mod zones;

#[cfg(feature = "grpc")]
pub mod transport;

#[cfg(feature = "wasm")]
pub mod wasm;

pub use vector::HammingVector;
pub use codebook::{Codebook, CodebookEntry};
pub use frame::{HammingFrame, FrameHeader, PayloadMode};
pub use xor_stream::XorStream;
pub use error::HammingError;

/// Fingerprint size in bits (compatible with ladybug-rs and firefly)
pub const FINGERPRINT_BITS: usize = 10_000;

/// Fingerprint size in bytes (ceil(10000/8) = 1250)
pub const FINGERPRINT_BYTES: usize = 1_250;

/// Fingerprint size in u64 words (ceil(10000/64) = 157, using 10048 bits)
pub const FINGERPRINT_WORDS: usize = 157;

/// Maximum codebook entries (12-bit address space)
pub const CODEBOOK_SIZE: usize = 4_096;

/// UDP frame header size
pub const FRAME_HEADER_SIZE: usize = 32;

/// HMAC tag size (truncated SHA-256)
pub const HMAC_TAG_SIZE: usize = 16;

/// Maximum UDP payload (1500 MTU - 20 IP - 8 UDP)
pub const MAX_UDP_PAYLOAD: usize = 1_472;

/// Maximum frame payload (after header and HMAC)
pub const MAX_FRAME_PAYLOAD: usize = MAX_UDP_PAYLOAD - FRAME_HEADER_SIZE - HMAC_TAG_SIZE;

/// Frame magic bytes
pub const FRAME_MAGIC: [u8; 2] = [0xA2, 0xB1];

/// Protocol version
pub const PROTOCOL_VERSION: u8 = 0x01;
