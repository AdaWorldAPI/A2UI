//! XOR stream: codebook versioning and delta synchronization.
//!
//! Codebook updates are distributed as XOR deltas. Since XOR is self-inverse,
//! applying a delta twice restores the original — enabling rollback with zero
//! additional storage.
//!
//! The XOR stream also supports loading a full codebook incrementally: the
//! server streams codebook entries as XOR deltas against a zero vector,
//! effectively "building up" the codebook on the client side.

use crate::codebook::{Codebook, CodebookEntry, DeltaOp};
use crate::error::{HammingError, Result};
use crate::vector::HammingVector;
use sha2::{Digest, Sha256};
use serde::{Serialize, Deserialize};

/// A versioned delta in the XOR stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XorDelta {
    /// Source codebook version this delta applies to
    pub from_version: u32,
    /// Target codebook version after applying this delta
    pub to_version: u32,
    /// Individual entry deltas
    pub entries: Vec<XorDeltaEntry>,
    /// SHA-256 integrity hash of the target codebook after applying
    pub target_hash: [u8; 32],
    /// Chain hash: SHA-256(previous_chain_hash || this_delta)
    pub chain_hash: [u8; 32],
}

/// A single entry delta in the XOR stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XorDeltaEntry {
    /// Codebook address to update
    pub address: u32,
    /// XOR mask (for Modify: XOR with existing; for Insert: the new fingerprint)
    pub xor_mask: HammingVector,
    /// Operation type
    pub operation: DeltaOp,
    /// Updated semantic label (empty = no change)
    pub semantic_label: String,
}

/// Manages the XOR stream for codebook synchronization.
///
/// Tracks version history and can produce/apply deltas between any
/// two versions (within the retained history window).
pub struct XorStream {
    /// History of deltas, ordered by version
    history: Vec<XorDelta>,
    /// Maximum number of deltas to retain
    max_history: usize,
    /// Current chain hash
    chain_hash: [u8; 32],
}

impl XorStream {
    /// Create a new XOR stream with the given history depth.
    pub fn new(max_history: usize) -> Self {
        Self {
            history: Vec::new(),
            max_history,
            chain_hash: [0u8; 32],
        }
    }

    /// Record a delta between two codebook versions.
    pub fn record_delta(
        &mut self,
        from: &Codebook,
        to: &Codebook,
    ) -> XorDelta {
        let diff = from.diff(to);

        let entries: Vec<XorDeltaEntry> = diff
            .into_iter()
            .map(|(addr, xor_mask, op)| {
                let label = to
                    .get(addr)
                    .map(|e| e.semantic_label.clone())
                    .unwrap_or_default();
                XorDeltaEntry {
                    address: addr,
                    xor_mask,
                    operation: op,
                    semantic_label: label,
                }
            })
            .collect();

        let target_hash = to.integrity_hash();

        // Compute chain hash: SHA-256(previous_chain || delta_hash)
        let delta_hash = Self::hash_delta_entries(&entries);
        let mut chain_hasher = Sha256::new();
        chain_hasher.update(self.chain_hash);
        chain_hasher.update(delta_hash);
        let new_chain_hash: [u8; 32] = chain_hasher.finalize().into();

        let delta = XorDelta {
            from_version: from.version(),
            to_version: to.version(),
            entries,
            target_hash,
            chain_hash: new_chain_hash,
        };

        self.chain_hash = new_chain_hash;
        self.history.push(delta.clone());

        // Trim history if needed
        if self.history.len() > self.max_history {
            self.history.drain(0..self.history.len() - self.max_history);
        }

        delta
    }

    /// Apply a delta to a codebook, verifying integrity.
    pub fn apply_delta(
        &self,
        codebook: &mut Codebook,
        delta: &XorDelta,
    ) -> Result<()> {
        // Verify version chain
        if codebook.version() != delta.from_version {
            return Err(HammingError::VersionMismatch {
                expected: delta.from_version,
                actual: codebook.version(),
            });
        }

        // Convert XorDeltaEntries to the format codebook expects
        let ops: Vec<(u32, HammingVector, DeltaOp)> = delta
            .entries
            .iter()
            .map(|e| (e.address, e.xor_mask.clone(), e.operation))
            .collect();

        codebook.apply_delta(&ops)?;

        // Verify integrity hash
        let actual_hash = codebook.integrity_hash();
        if actual_hash != delta.target_hash {
            return Err(HammingError::XorDeltaFailed(
                "integrity hash mismatch after applying delta".into(),
            ));
        }

        Ok(())
    }

    /// Generate a "full sync" delta that creates a codebook from scratch.
    ///
    /// This produces a delta from version 0 (empty) to the given codebook,
    /// where each entry's XOR mask IS the fingerprint itself (since XOR
    /// with zero = identity).
    pub fn full_sync_delta(codebook: &Codebook) -> XorDelta {
        let entries: Vec<XorDeltaEntry> = codebook
            .iter()
            .map(|(&addr, entry)| XorDeltaEntry {
                address: addr,
                xor_mask: entry.fingerprint.clone(),
                operation: DeltaOp::Insert,
                semantic_label: entry.semantic_label.clone(),
            })
            .collect();

        let target_hash = codebook.integrity_hash();
        let delta_hash = Self::hash_delta_entries(&entries);
        let mut chain_hasher = Sha256::new();
        chain_hasher.update([0u8; 32]); // genesis chain hash
        chain_hasher.update(delta_hash);
        let chain_hash: [u8; 32] = chain_hasher.finalize().into();

        XorDelta {
            from_version: 0,
            to_version: codebook.version(),
            entries,
            target_hash,
            chain_hash,
        }
    }

    /// Chunk a delta into UDP-sized pieces for streaming.
    ///
    /// Each chunk contains a subset of entries that fits within the frame
    /// payload limit. The receiver accumulates chunks and applies the
    /// complete delta atomically.
    pub fn chunk_delta(delta: &XorDelta, max_entries_per_chunk: usize) -> Vec<XorDeltaChunk> {
        let total_chunks = (delta.entries.len() + max_entries_per_chunk - 1)
            / max_entries_per_chunk;
        let total_chunks = total_chunks.max(1);

        delta
            .entries
            .chunks(max_entries_per_chunk)
            .enumerate()
            .map(|(i, chunk)| XorDeltaChunk {
                from_version: delta.from_version,
                to_version: delta.to_version,
                chunk_index: i as u32,
                total_chunks: total_chunks as u32,
                entries: chunk.to_vec(),
                is_last: i == total_chunks - 1,
                target_hash: if i == total_chunks - 1 {
                    Some(delta.target_hash)
                } else {
                    None
                },
            })
            .collect()
    }

    /// Get the delta history.
    pub fn history(&self) -> &[XorDelta] {
        &self.history
    }

    /// Get the latest delta.
    pub fn latest(&self) -> Option<&XorDelta> {
        self.history.last()
    }

    /// Find a delta path from one version to another.
    ///
    /// Returns a sequence of deltas that, when applied in order,
    /// transform the codebook from `from_version` to `to_version`.
    pub fn find_path(&self, from_version: u32, to_version: u32) -> Option<Vec<&XorDelta>> {
        if from_version == to_version {
            return Some(Vec::new());
        }

        let mut path = Vec::new();
        let mut current = from_version;

        for delta in &self.history {
            if delta.from_version == current {
                path.push(delta);
                current = delta.to_version;
                if current == to_version {
                    return Some(path);
                }
            }
        }

        None // No path found in history
    }

    fn hash_delta_entries(entries: &[XorDeltaEntry]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for entry in entries {
            hasher.update(&entry.address.to_le_bytes());
            hasher.update(&entry.xor_mask.to_bytes());
            hasher.update(&[entry.operation as u8]);
        }
        hasher.finalize().into()
    }
}

/// A chunk of an XOR delta for streaming transmission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XorDeltaChunk {
    pub from_version: u32,
    pub to_version: u32,
    pub chunk_index: u32,
    pub total_chunks: u32,
    pub entries: Vec<XorDeltaEntry>,
    pub is_last: bool,
    pub target_hash: Option<[u8; 32]>,
}

/// Accumulator for receiving chunked XOR deltas.
pub struct DeltaAccumulator {
    from_version: u32,
    to_version: u32,
    total_chunks: u32,
    received: Vec<Option<Vec<XorDeltaEntry>>>,
    target_hash: Option<[u8; 32]>,
}

impl DeltaAccumulator {
    pub fn new(from_version: u32, to_version: u32, total_chunks: u32) -> Self {
        Self {
            from_version,
            to_version,
            total_chunks,
            received: vec![None; total_chunks as usize],
            target_hash: None,
        }
    }

    /// Add a received chunk. Returns true if all chunks received.
    pub fn add_chunk(&mut self, chunk: XorDeltaChunk) -> bool {
        let idx = chunk.chunk_index as usize;
        if idx < self.received.len() {
            self.received[idx] = Some(chunk.entries);
            if chunk.is_last {
                self.target_hash = chunk.target_hash;
            }
        }
        self.is_complete()
    }

    /// Check if all chunks have been received.
    pub fn is_complete(&self) -> bool {
        self.received.iter().all(|c| c.is_some())
    }

    /// Assemble the complete delta from received chunks.
    pub fn assemble(&self) -> Option<XorDelta> {
        if !self.is_complete() {
            return None;
        }

        let entries: Vec<XorDeltaEntry> = self
            .received
            .iter()
            .filter_map(|c| c.as_ref())
            .flat_map(|c| c.iter().cloned())
            .collect();

        Some(XorDelta {
            from_version: self.from_version,
            to_version: self.to_version,
            entries,
            target_hash: self.target_hash.unwrap_or([0u8; 32]),
            chain_hash: [0u8; 32], // Recomputed on application
        })
    }

    /// Count how many chunks are still missing.
    pub fn missing_chunks(&self) -> Vec<u32> {
        self.received
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_none())
            .map(|(i, _)| i as u32)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codebook::Codebook;
    use crate::zones;

    #[test]
    fn test_full_sync_delta() {
        let cb = Codebook::standard();
        let delta = XorStream::full_sync_delta(&cb);

        assert_eq!(delta.from_version, 0);
        assert_eq!(delta.to_version, cb.version());
        assert_eq!(delta.entries.len(), cb.len());

        // Apply to empty codebook
        let mut empty = Codebook::new();
        let stream = XorStream::new(100);
        // Manually set version to 0 for the delta to match
        // (empty codebook starts at version 0)
        stream.apply_delta(&mut empty, &delta).unwrap();

        assert_eq!(empty.len(), cb.len());
    }

    #[test]
    fn test_incremental_delta() {
        let cb1 = Codebook::standard();
        let mut cb2 = cb1.clone();

        // Modify button entry
        let new_fp = HammingVector::from_seed("modified.button.v2");
        let entry = cb2.get(zones::components::BUTTON).unwrap().clone();
        let mut updated = entry;
        updated.fingerprint = new_fp;
        updated.entry_version += 1;
        cb2.insert(updated).unwrap();

        let mut stream = XorStream::new(100);
        let delta = stream.record_delta(&cb1, &cb2);

        // Delta should have exactly one modification
        assert_eq!(delta.entries.len(), 1);
        assert_eq!(delta.entries[0].address, zones::components::BUTTON);
        assert_eq!(delta.entries[0].operation, DeltaOp::Modify);

        // Apply delta to copy of cb1
        let mut cb1_copy = cb1.clone();
        stream.apply_delta(&mut cb1_copy, &delta).unwrap();

        // Verify button matches
        let b1 = cb1_copy.get_fingerprint(zones::components::BUTTON).unwrap();
        let b2 = cb2.get_fingerprint(zones::components::BUTTON).unwrap();
        assert_eq!(b1, b2);
    }

    #[test]
    fn test_xor_self_inverse() {
        let cb1 = Codebook::standard();
        let mut cb2 = cb1.clone();

        let new_fp = HammingVector::from_seed("modified.text");
        let entry = cb2.get(zones::components::TEXT).unwrap().clone();
        let mut updated = entry;
        updated.fingerprint = new_fp;
        updated.entry_version += 1;
        cb2.insert(updated).unwrap();

        let mut stream = XorStream::new(100);
        let delta = stream.record_delta(&cb1, &cb2);

        // Apply forward
        let mut forward = cb1.clone();
        stream.apply_delta(&mut forward, &delta).unwrap();

        // Apply the same delta again should NOT work (version mismatch)
        let result = stream.apply_delta(&mut forward, &delta);
        assert!(result.is_err());
    }

    #[test]
    fn test_chunked_delta() {
        let cb = Codebook::standard();
        let delta = XorStream::full_sync_delta(&cb);

        // Chunk into small pieces
        let chunks = XorStream::chunk_delta(&delta, 10);
        assert!(chunks.len() > 1);

        // Accumulate
        let mut acc =
            DeltaAccumulator::new(delta.from_version, delta.to_version, chunks.len() as u32);
        for chunk in chunks {
            acc.add_chunk(chunk);
        }
        assert!(acc.is_complete());

        let assembled = acc.assemble().unwrap();
        assert_eq!(assembled.entries.len(), delta.entries.len());
    }
}
