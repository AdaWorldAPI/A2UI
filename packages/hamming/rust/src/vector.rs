//! 10,000-bit Hamming fingerprint vector.
//!
//! Binary-compatible with ladybug-rs `fingerprint()` and firefly `HammingVector`.
//! Stored as 157 × u64 words (10,048 bits, top 48 unused).

use crate::error::{HammingError, Result};
use crate::{FINGERPRINT_BITS, FINGERPRINT_BYTES, FINGERPRINT_WORDS};
use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use std::ops::{BitXor, BitAnd, BitOr, Not};

/// A 10,000-bit fingerprint stored as 157 u64 words.
///
/// Compatible with ladybug-rs core::fingerprint and firefly vector::HammingVector.
/// The top 48 bits of the last word are always zero.
#[derive(Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct HammingVector {
    words: [u64; FINGERPRINT_WORDS],
}

impl HammingVector {
    /// Create a zero vector.
    pub fn zero() -> Self {
        Self {
            words: [0u64; FINGERPRINT_WORDS],
        }
    }

    /// Create from raw u64 words.
    pub fn from_words(words: [u64; FINGERPRINT_WORDS]) -> Self {
        let mut v = Self { words };
        v.mask_unused_bits();
        v
    }

    /// Create from bytes (1,250 bytes = 10,000 bits).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != FINGERPRINT_BYTES {
            return Err(HammingError::InvalidFingerprintLength {
                expected: FINGERPRINT_BYTES,
                actual: bytes.len(),
            });
        }

        let mut words = [0u64; FINGERPRINT_WORDS];
        for (i, word) in words.iter_mut().enumerate() {
            let start = i * 8;
            let end = (start + 8).min(bytes.len());
            let mut buf = [0u8; 8];
            let len = end - start;
            if len > 0 {
                buf[..len].copy_from_slice(&bytes[start..end]);
            }
            *word = u64::from_le_bytes(buf);
        }

        let mut v = Self { words };
        v.mask_unused_bits();
        Ok(v)
    }

    /// Serialize to 1,250 bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(FINGERPRINT_BYTES);
        for (i, &word) in self.words.iter().enumerate() {
            let word_bytes = word.to_le_bytes();
            let remaining = FINGERPRINT_BYTES.saturating_sub(i * 8);
            let take = remaining.min(8);
            bytes.extend_from_slice(&word_bytes[..take]);
        }
        bytes.truncate(FINGERPRINT_BYTES);
        bytes
    }

    /// Create a deterministic fingerprint from a string seed.
    ///
    /// Uses SHA-256 seeded LFSR expansion, compatible with ladybug-rs.
    /// This is the core content-addressing function.
    pub fn from_seed(seed: &str) -> Self {
        let hash = Sha256::digest(seed.as_bytes());
        let mut words = [0u64; FINGERPRINT_WORDS];

        // Use SHA-256 hash as LFSR seed, expand to 10,000 bits
        let mut state = u64::from_le_bytes(hash[0..8].try_into().unwrap());
        let feedback = u64::from_le_bytes(hash[8..16].try_into().unwrap()) | 1;

        for word in words.iter_mut() {
            // LFSR step with SHA-256-derived feedback polynomial
            let mut val = 0u64;
            for bit in 0..64 {
                let output = state & 1;
                val |= output << bit;
                let new_bit = (state ^ (state >> 1) ^ (state >> 3) ^ (state >> 7)) & 1;
                state = (state >> 1) | (new_bit << 63);
                state ^= feedback.wrapping_mul(output);
            }
            *word = val;
        }

        let mut v = Self { words };
        v.mask_unused_bits();
        v
    }

    /// Compute Hamming distance to another vector.
    ///
    /// Returns the number of differing bits (0 to 10,000).
    pub fn hamming_distance(&self, other: &Self) -> u32 {
        #[cfg(all(feature = "simd", target_arch = "x86_64"))]
        {
            self.hamming_distance_simd(other)
        }
        #[cfg(not(all(feature = "simd", target_arch = "x86_64")))]
        {
            self.hamming_distance_scalar(other)
        }
    }

    /// Scalar Hamming distance using CPU POPCNT.
    fn hamming_distance_scalar(&self, other: &Self) -> u32 {
        self.words
            .iter()
            .zip(other.words.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }

    /// SIMD-accelerated Hamming distance (x86_64 with POPCNT).
    #[cfg(all(feature = "simd", target_arch = "x86_64"))]
    fn hamming_distance_simd(&self, other: &Self) -> u32 {
        // Use compiler auto-vectorization with explicit POPCNT
        self.words
            .iter()
            .zip(other.words.iter())
            .map(|(a, b)| {
                let xor = a ^ b;
                xor.count_ones()
            })
            .sum()
    }

    /// Normalized similarity (0.0 = completely different, 1.0 = identical).
    pub fn similarity(&self, other: &Self) -> f64 {
        1.0 - (self.hamming_distance(other) as f64 / FINGERPRINT_BITS as f64)
    }

    /// Count the number of set bits.
    pub fn popcount(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    /// Majority vote across multiple vectors (bundle operation).
    ///
    /// For each bit position, the result bit is 1 if more than half
    /// of the input vectors have a 1 at that position.
    pub fn bundle(vectors: &[&Self]) -> Self {
        if vectors.is_empty() {
            return Self::zero();
        }
        if vectors.len() == 1 {
            return vectors[0].clone();
        }

        let threshold = vectors.len() / 2;
        let mut result = [0u64; FINGERPRINT_WORDS];

        for bit_word in 0..FINGERPRINT_WORDS {
            let mut word = 0u64;
            for bit in 0..64 {
                let count: usize = vectors
                    .iter()
                    .filter(|v| (v.words[bit_word] >> bit) & 1 == 1)
                    .count();
                if count > threshold {
                    word |= 1u64 << bit;
                }
            }
            result[bit_word] = word;
        }

        let mut v = Self { words: result };
        v.mask_unused_bits();
        v
    }

    /// Access raw words (for Arrow Flight serialization).
    pub fn as_words(&self) -> &[u64; FINGERPRINT_WORDS] {
        &self.words
    }

    /// Mask the unused top 48 bits of the last word to ensure canonical form.
    fn mask_unused_bits(&mut self) {
        // 10,000 bits = 156 full words (9,984 bits) + 16 bits in word 156
        // So only the bottom 16 bits of words[156] are used
        let used_bits_last = FINGERPRINT_BITS - (FINGERPRINT_WORDS - 1) * 64;
        if used_bits_last < 64 {
            self.words[FINGERPRINT_WORDS - 1] &= (1u64 << used_bits_last) - 1;
        }
    }
}

impl BitXor for HammingVector {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self {
        let mut words = [0u64; FINGERPRINT_WORDS];
        for i in 0..FINGERPRINT_WORDS {
            words[i] = self.words[i] ^ rhs.words[i];
        }
        let mut v = Self { words };
        v.mask_unused_bits();
        v
    }
}

impl BitXor for &HammingVector {
    type Output = HammingVector;

    fn bitxor(self, rhs: Self) -> HammingVector {
        let mut words = [0u64; FINGERPRINT_WORDS];
        for i in 0..FINGERPRINT_WORDS {
            words[i] = self.words[i] ^ rhs.words[i];
        }
        let mut v = HammingVector { words };
        v.mask_unused_bits();
        v
    }
}

impl BitAnd for &HammingVector {
    type Output = HammingVector;

    fn bitand(self, rhs: Self) -> HammingVector {
        let mut words = [0u64; FINGERPRINT_WORDS];
        for i in 0..FINGERPRINT_WORDS {
            words[i] = self.words[i] & rhs.words[i];
        }
        HammingVector { words }
    }
}

impl BitOr for &HammingVector {
    type Output = HammingVector;

    fn bitor(self, rhs: Self) -> HammingVector {
        let mut words = [0u64; FINGERPRINT_WORDS];
        for i in 0..FINGERPRINT_WORDS {
            words[i] = self.words[i] | rhs.words[i];
        }
        let mut v = HammingVector { words };
        v.mask_unused_bits();
        v
    }
}

impl Not for &HammingVector {
    type Output = HammingVector;

    fn not(self) -> HammingVector {
        let mut words = [0u64; FINGERPRINT_WORDS];
        for i in 0..FINGERPRINT_WORDS {
            words[i] = !self.words[i];
        }
        let mut v = HammingVector { words };
        v.mask_unused_bits();
        v
    }
}

impl std::fmt::Debug for HammingVector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.to_bytes();
        let hex_prefix: String = bytes.iter().take(8).map(|b| format!("{:02x}", b)).collect();
        write!(
            f,
            "HammingVector({}...  popcount={}, {} bits)",
            hex_prefix,
            self.popcount(),
            FINGERPRINT_BITS
        )
    }
}

impl std::fmt::Display for HammingVector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.to_bytes();
        write!(f, "{}", hex::encode(&bytes[..16]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_vector() {
        let v = HammingVector::zero();
        assert_eq!(v.popcount(), 0);
        assert_eq!(v.to_bytes().len(), FINGERPRINT_BYTES);
    }

    #[test]
    fn test_from_seed_deterministic() {
        let a = HammingVector::from_seed("a2ui.component.Button");
        let b = HammingVector::from_seed("a2ui.component.Button");
        assert_eq!(a, b);
        assert_eq!(a.hamming_distance(&b), 0);
    }

    #[test]
    fn test_different_seeds_differ() {
        let button = HammingVector::from_seed("a2ui.component.Button");
        let card = HammingVector::from_seed("a2ui.component.Card");
        assert_ne!(button, card);
        // Should differ by roughly 50% of bits (random-like)
        let dist = button.hamming_distance(&card);
        assert!(dist > 4000 && dist < 6000, "distance={}", dist);
    }

    #[test]
    fn test_xor_self_inverse() {
        let a = HammingVector::from_seed("hello");
        let b = HammingVector::from_seed("world");
        let composed = &a ^ &b;
        let recovered = &composed ^ &b;
        assert_eq!(a, recovered);
    }

    #[test]
    fn test_bytes_roundtrip() {
        let v = HammingVector::from_seed("roundtrip test");
        let bytes = v.to_bytes();
        assert_eq!(bytes.len(), FINGERPRINT_BYTES);
        let v2 = HammingVector::from_bytes(&bytes).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn test_similarity() {
        let a = HammingVector::from_seed("same");
        assert_eq!(a.similarity(&a), 1.0);

        let b = HammingVector::from_seed("different");
        let sim = a.similarity(&b);
        assert!(sim > 0.4 && sim < 0.6, "similarity={}", sim);
    }

    #[test]
    fn test_bundle() {
        let a = HammingVector::from_seed("alpha");
        let b = HammingVector::from_seed("beta");
        let c = HammingVector::from_seed("alpha"); // same as a
        let bundled = HammingVector::bundle(&[&a, &b, &c]);
        // Bundled should be closer to a (appears twice) than to b
        assert!(bundled.similarity(&a) > bundled.similarity(&b));
    }

    #[test]
    fn test_invalid_bytes_length() {
        let result = HammingVector::from_bytes(&[0u8; 100]);
        assert!(result.is_err());
    }
}
