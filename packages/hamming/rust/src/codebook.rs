//! Codebook: B-tree indexed dictionary of 10kbit fingerprints.
//!
//! Maps 12-bit addresses (0x000–0xFFF) to HammingVector fingerprints.
//! Supports XOR composition, version tracking, and nearest-neighbor search.

use crate::error::{HammingError, Result};
use crate::vector::HammingVector;
use crate::zones::{self, zone_for_address, Zone};
use crate::CODEBOOK_SIZE;

use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

/// A single codebook entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodebookEntry {
    /// 12-bit address (0x000–0xFFF)
    pub address: u32,
    /// 10,000-bit fingerprint
    pub fingerprint: HammingVector,
    /// Zone this entry belongs to
    pub zone: &'static str,
    /// Entry version (incremented on update)
    pub entry_version: u32,
    /// Human-readable semantic label (debug only)
    pub semantic_label: String,
    /// Related entry addresses (for cognitive lookup)
    pub related: Vec<u32>,
}

/// The codebook: a B-tree indexed map of addresses to fingerprints.
///
/// B-tree provides O(log n) lookup and ordered iteration, which enables
/// efficient range queries over address zones.
#[derive(Debug, Clone)]
pub struct Codebook {
    /// B-tree mapping addresses to entries
    entries: BTreeMap<u32, CodebookEntry>,
    /// Global codebook version (monotonic)
    version: u32,
    /// Prefix index: first 64 bits of fingerprint → address
    /// for fast fingerprint-to-address reverse lookup
    prefix_index: BTreeMap<u64, Vec<u32>>,
}

impl Codebook {
    /// Create an empty codebook.
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            version: 0,
            prefix_index: BTreeMap::new(),
        }
    }

    /// Create a codebook pre-populated with standard A2UI entries.
    ///
    /// Generates deterministic fingerprints for all well-known addresses
    /// in the protocol, message, component, style, and action zones.
    pub fn standard() -> Self {
        let mut cb = Self::new();
        cb.populate_standard_entries();
        cb
    }

    /// Current codebook version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up an entry by address.
    pub fn get(&self, address: u32) -> Option<&CodebookEntry> {
        self.entries.get(&address)
    }

    /// Look up a fingerprint by address.
    pub fn get_fingerprint(&self, address: u32) -> Option<&HammingVector> {
        self.entries.get(&address).map(|e| &e.fingerprint)
    }

    /// Insert or update an entry. Increments codebook version.
    pub fn insert(&mut self, entry: CodebookEntry) -> Result<()> {
        if entry.address >= CODEBOOK_SIZE as u32 {
            return Err(HammingError::AddressOutOfRange {
                address: entry.address,
            });
        }

        // Update prefix index
        let prefix = Self::fingerprint_prefix(&entry.fingerprint);
        self.prefix_index
            .entry(prefix)
            .or_default()
            .push(entry.address);

        self.entries.insert(entry.address, entry);
        self.version += 1;
        Ok(())
    }

    /// Remove an entry. Increments codebook version.
    pub fn remove(&mut self, address: u32) -> Option<CodebookEntry> {
        if let Some(entry) = self.entries.remove(&address) {
            // Clean prefix index
            let prefix = Self::fingerprint_prefix(&entry.fingerprint);
            if let Some(addrs) = self.prefix_index.get_mut(&prefix) {
                addrs.retain(|&a| a != address);
                if addrs.is_empty() {
                    self.prefix_index.remove(&prefix);
                }
            }
            self.version += 1;
            Some(entry)
        } else {
            None
        }
    }

    /// Find the nearest codebook entry by Hamming distance.
    ///
    /// Returns (address, distance) of the closest match.
    pub fn nearest(&self, query: &HammingVector) -> Option<(u32, u32)> {
        self.entries
            .values()
            .map(|e| (e.address, query.hamming_distance(&e.fingerprint)))
            .min_by_key(|&(_, dist)| dist)
    }

    /// Find the nearest entry within a specific zone.
    pub fn nearest_in_zone(&self, query: &HammingVector, zone: &Zone) -> Option<(u32, u32)> {
        self.entries
            .range(zone.range.clone())
            .map(|(_, e)| (e.address, query.hamming_distance(&e.fingerprint)))
            .min_by_key(|&(_, dist)| dist)
    }

    /// Find all entries within a Hamming distance threshold.
    pub fn search(&self, query: &HammingVector, max_distance: u32) -> Vec<(u32, u32)> {
        let mut results: Vec<_> = self
            .entries
            .values()
            .map(|e| (e.address, query.hamming_distance(&e.fingerprint)))
            .filter(|&(_, dist)| dist <= max_distance)
            .collect();
        results.sort_by_key(|&(_, dist)| dist);
        results
    }

    /// Resolve an XOR composition chain to a fingerprint.
    ///
    /// Looks up each address and XORs the fingerprints together.
    pub fn resolve_xor_chain(&self, addresses: &[u32]) -> Result<HammingVector> {
        if addresses.is_empty() {
            return Ok(HammingVector::zero());
        }

        let first = self
            .get_fingerprint(addresses[0])
            .ok_or(HammingError::EntryNotFound {
                address: addresses[0],
            })?
            .clone();

        addresses[1..].iter().try_fold(first, |acc, &addr| {
            let fp = self
                .get_fingerprint(addr)
                .ok_or(HammingError::EntryNotFound { address: addr })?;
            Ok(&acc ^ fp)
        })
    }

    /// Apply an XOR delta to update the codebook.
    ///
    /// For each delta: `codebook[addr] = codebook[addr] ⊕ xor_mask`
    /// This is self-inverse: applying the same delta twice restores the original.
    pub fn apply_delta(&mut self, deltas: &[(u32, HammingVector, DeltaOp)]) -> Result<()> {
        for (address, xor_mask, op) in deltas {
            match op {
                DeltaOp::Modify => {
                    let entry = self.entries.get_mut(address).ok_or(
                        HammingError::EntryNotFound { address: *address },
                    )?;
                    entry.fingerprint = &entry.fingerprint ^ xor_mask;
                    entry.entry_version += 1;
                }
                DeltaOp::Insert => {
                    let zone_name = zone_for_address(*address)
                        .map(|z| z.name)
                        .unwrap_or("unknown");
                    self.insert(CodebookEntry {
                        address: *address,
                        fingerprint: xor_mask.clone(),
                        zone: zone_name,
                        entry_version: 0,
                        semantic_label: String::new(),
                        related: Vec::new(),
                    })?;
                }
                DeltaOp::Delete => {
                    self.remove(*address);
                }
            }
        }
        self.version += 1;
        Ok(())
    }

    /// Compute an XOR delta between this codebook and another version.
    ///
    /// Returns the minimal set of deltas needed to transform `self` into `other`.
    pub fn diff(&self, other: &Codebook) -> Vec<(u32, HammingVector, DeltaOp)> {
        let mut deltas = Vec::new();

        // Entries modified or deleted
        for (&address, entry) in &self.entries {
            match other.entries.get(&address) {
                Some(other_entry) => {
                    if entry.fingerprint != other_entry.fingerprint {
                        let xor = &entry.fingerprint ^ &other_entry.fingerprint;
                        deltas.push((address, xor, DeltaOp::Modify));
                    }
                }
                None => {
                    deltas.push((address, HammingVector::zero(), DeltaOp::Delete));
                }
            }
        }

        // New entries in other
        for (&address, entry) in &other.entries {
            if !self.entries.contains_key(&address) {
                deltas.push((address, entry.fingerprint.clone(), DeltaOp::Insert));
            }
        }

        deltas
    }

    /// Get all entries in a zone.
    pub fn zone_entries(&self, zone: &Zone) -> Vec<&CodebookEntry> {
        self.entries
            .range(zone.range.clone())
            .map(|(_, e)| e)
            .collect()
    }

    /// Iterate over all entries in address order.
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &CodebookEntry)> {
        self.entries.iter()
    }

    /// Compute SHA-256 integrity hash over all entries (sorted by address).
    pub fn integrity_hash(&self) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        for (_, entry) in &self.entries {
            hasher.update(&entry.address.to_le_bytes());
            hasher.update(&entry.fingerprint.to_bytes());
        }
        hasher.finalize().into()
    }

    /// Serialize the full codebook to bytes (for Arrow Flight transport).
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(&SerializableCodebook::from(self))
            .unwrap_or_default()
    }

    /// Deserialize a codebook from bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let scb: SerializableCodebook = bincode::deserialize(data)
            .map_err(|e| HammingError::SerializationError(e.to_string()))?;
        Ok(scb.into())
    }

    // ── Private helpers ──────────────────────────────────────

    /// Extract the first 64 bits of a fingerprint for the prefix index.
    fn fingerprint_prefix(fp: &HammingVector) -> u64 {
        fp.as_words()[0]
    }

    /// Populate standard entries for all well-known addresses.
    fn populate_standard_entries(&mut self) {
        // Protocol zone
        self.insert_standard(zones::protocol::HANDSHAKE_INIT, "protocol", "handshake.init");
        self.insert_standard(zones::protocol::HANDSHAKE_ACK, "protocol", "handshake.ack");
        self.insert_standard(zones::protocol::HEARTBEAT, "protocol", "heartbeat");
        self.insert_standard(zones::protocol::CODEBOOK_SYNC, "protocol", "codebook.sync");
        self.insert_standard(zones::protocol::SESSION_CLOSE, "protocol", "session.close");
        self.insert_standard(zones::protocol::ERROR, "protocol", "error");
        self.insert_standard(zones::protocol::VERSION_ANNOUNCE, "protocol", "version.announce");
        self.insert_standard(zones::protocol::NOP, "protocol", "nop");

        // Message zone
        self.insert_standard(zones::messages::CREATE_SURFACE, "messages", "a2ui.message.createSurface");
        self.insert_standard(zones::messages::UPDATE_COMPONENTS, "messages", "a2ui.message.updateComponents");
        self.insert_standard(zones::messages::UPDATE_DATA_MODEL, "messages", "a2ui.message.updateDataModel");
        self.insert_standard(zones::messages::DELETE_SURFACE, "messages", "a2ui.message.deleteSurface");
        self.insert_standard(zones::messages::ATTACH_DATA_MODEL, "messages", "a2ui.message.attachDataModel");
        self.insert_standard(zones::messages::BEGIN_RENDERING, "messages", "a2ui.message.beginRendering");
        self.insert_standard(zones::messages::SURFACE_UPDATE, "messages", "a2ui.message.surfaceUpdate");

        // Component zone
        self.insert_standard(zones::components::TEXT, "components", "a2ui.component.Text");
        self.insert_standard(zones::components::BUTTON, "components", "a2ui.component.Button");
        self.insert_standard(zones::components::CARD, "components", "a2ui.component.Card");
        self.insert_standard(zones::components::ROW, "components", "a2ui.component.Row");
        self.insert_standard(zones::components::COLUMN, "components", "a2ui.component.Column");
        self.insert_standard(zones::components::IMAGE, "components", "a2ui.component.Image");
        self.insert_standard(zones::components::TEXT_FIELD, "components", "a2ui.component.TextField");
        self.insert_standard(zones::components::SLIDER, "components", "a2ui.component.Slider");
        self.insert_standard(zones::components::CHECKBOX, "components", "a2ui.component.Checkbox");
        self.insert_standard(zones::components::MODAL, "components", "a2ui.component.Modal");
        self.insert_standard(zones::components::TABS, "components", "a2ui.component.Tabs");
        self.insert_standard(zones::components::LIST, "components", "a2ui.component.List");
        self.insert_standard(zones::components::DIVIDER, "components", "a2ui.component.Divider");
        self.insert_standard(zones::components::ICON, "components", "a2ui.component.Icon");
        self.insert_standard(zones::components::VIDEO, "components", "a2ui.component.Video");
        self.insert_standard(zones::components::AUDIO, "components", "a2ui.component.Audio");
        self.insert_standard(zones::components::DATE_TIME_INPUT, "components", "a2ui.component.DateTimeInput");
        self.insert_standard(zones::components::MULTIPLE_CHOICE, "components", "a2ui.component.MultipleChoice");
        self.insert_standard(zones::components::MARKDOWN, "components", "a2ui.component.Markdown");
        self.insert_standard(zones::components::PROGRESS, "components", "a2ui.component.Progress");

        // Style zone
        self.insert_standard(zones::styles::THEME_LIGHT, "styles", "a2ui.style.theme.light");
        self.insert_standard(zones::styles::THEME_DARK, "styles", "a2ui.style.theme.dark");
        self.insert_standard(zones::styles::LAYOUT_FLEX_ROW, "styles", "a2ui.style.layout.flexRow");
        self.insert_standard(zones::styles::LAYOUT_FLEX_COLUMN, "styles", "a2ui.style.layout.flexColumn");
        self.insert_standard(zones::styles::LAYOUT_GRID, "styles", "a2ui.style.layout.grid");
        self.insert_standard(zones::styles::SPACING_SMALL, "styles", "a2ui.style.spacing.small");
        self.insert_standard(zones::styles::SPACING_MEDIUM, "styles", "a2ui.style.spacing.medium");
        self.insert_standard(zones::styles::SPACING_LARGE, "styles", "a2ui.style.spacing.large");
        self.insert_standard(zones::styles::COLOR_PRIMARY, "styles", "a2ui.style.color.primary");
        self.insert_standard(zones::styles::COLOR_SECONDARY, "styles", "a2ui.style.color.secondary");
        self.insert_standard(zones::styles::COLOR_ERROR, "styles", "a2ui.style.color.error");
        self.insert_standard(zones::styles::COLOR_SUCCESS, "styles", "a2ui.style.color.success");
        self.insert_standard(zones::styles::FONT_HEADING, "styles", "a2ui.style.font.heading");
        self.insert_standard(zones::styles::FONT_BODY, "styles", "a2ui.style.font.body");
        self.insert_standard(zones::styles::FONT_MONO, "styles", "a2ui.style.font.mono");

        // Action zone
        self.insert_standard(zones::actions::CLICK, "actions", "a2ui.action.click");
        self.insert_standard(zones::actions::TEXT_INPUT, "actions", "a2ui.action.textInput");
        self.insert_standard(zones::actions::SCROLL, "actions", "a2ui.action.scroll");
        self.insert_standard(zones::actions::FOCUS, "actions", "a2ui.action.focus");
        self.insert_standard(zones::actions::BLUR, "actions", "a2ui.action.blur");
        self.insert_standard(zones::actions::SUBMIT, "actions", "a2ui.action.submit");
        self.insert_standard(zones::actions::NAVIGATE, "actions", "a2ui.action.navigate");
        self.insert_standard(zones::actions::SELECT, "actions", "a2ui.action.select");
        self.insert_standard(zones::actions::DRAG, "actions", "a2ui.action.drag");
        self.insert_standard(zones::actions::DROP, "actions", "a2ui.action.drop");

        // Cognitive zone
        self.insert_standard(zones::cognitive::NARS_QUERY, "cognitive", "a2ui.cognitive.nars.query");
        self.insert_standard(zones::cognitive::NARS_ASSERT, "cognitive", "a2ui.cognitive.nars.assert");
        self.insert_standard(zones::cognitive::NARS_INFER, "cognitive", "a2ui.cognitive.nars.infer");
        self.insert_standard(zones::cognitive::SEMANTIC_SEARCH, "cognitive", "a2ui.cognitive.semanticSearch");
        self.insert_standard(zones::cognitive::OPTIMIZE_REQUEST, "cognitive", "a2ui.cognitive.optimize");
        self.insert_standard(zones::cognitive::PATTERN_LEARNED, "cognitive", "a2ui.cognitive.patternLearned");
        self.insert_standard(zones::cognitive::PREDICTIVE_PREFETCH, "cognitive", "a2ui.cognitive.prefetch");
        self.insert_standard(zones::cognitive::CODEBOOK_REORGANIZE, "cognitive", "a2ui.cognitive.reorganize");

        // Reset version after initial population (these are baseline, not updates)
        self.version = 1;
    }

    fn insert_standard(&mut self, address: u32, zone: &'static str, seed: &str) {
        let _ = self.insert(CodebookEntry {
            address,
            fingerprint: HammingVector::from_seed(seed),
            zone,
            entry_version: 0,
            semantic_label: seed.to_string(),
            related: Vec::new(),
        });
    }
}

impl Default for Codebook {
    fn default() -> Self {
        Self::new()
    }
}

/// Operation type for codebook deltas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeltaOp {
    Modify,
    Insert,
    Delete,
}

/// Serializable version of Codebook (for bincode/Arrow transport).
#[derive(Serialize, Deserialize)]
struct SerializableCodebook {
    entries: Vec<SerializableEntry>,
    version: u32,
}

#[derive(Serialize, Deserialize)]
struct SerializableEntry {
    address: u32,
    fingerprint: HammingVector,
    zone: String,
    entry_version: u32,
    semantic_label: String,
    related: Vec<u32>,
}

impl From<&Codebook> for SerializableCodebook {
    fn from(cb: &Codebook) -> Self {
        Self {
            entries: cb
                .entries
                .values()
                .map(|e| SerializableEntry {
                    address: e.address,
                    fingerprint: e.fingerprint.clone(),
                    zone: e.zone.to_string(),
                    entry_version: e.entry_version,
                    semantic_label: e.semantic_label.clone(),
                    related: e.related.clone(),
                })
                .collect(),
            version: cb.version,
        }
    }
}

impl From<SerializableCodebook> for Codebook {
    fn from(scb: SerializableCodebook) -> Self {
        let mut cb = Codebook::new();
        for e in scb.entries {
            let zone_name = zone_for_address(e.address)
                .map(|z| z.name)
                .unwrap_or("unknown");
            let _ = cb.insert(CodebookEntry {
                address: e.address,
                fingerprint: e.fingerprint,
                zone: zone_name,
                entry_version: e.entry_version,
                semantic_label: e.semantic_label,
                related: e.related,
            });
        }
        cb.version = scb.version;
        cb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_codebook() {
        let cb = Codebook::standard();
        assert!(cb.len() > 50, "standard codebook should have many entries");
        assert_eq!(cb.version(), 1);

        // Verify well-known entries exist
        assert!(cb.get(zones::messages::CREATE_SURFACE).is_some());
        assert!(cb.get(zones::components::BUTTON).is_some());
        assert!(cb.get(zones::styles::THEME_DARK).is_some());
        assert!(cb.get(zones::actions::CLICK).is_some());
    }

    #[test]
    fn test_xor_composition() {
        let cb = Codebook::standard();

        // Compose createSurface + Column + theme:dark
        let result = cb
            .resolve_xor_chain(&[
                zones::messages::CREATE_SURFACE,
                zones::components::COLUMN,
                zones::styles::THEME_DARK,
            ])
            .unwrap();

        // The composed fingerprint should be different from any individual entry
        let create_fp = cb.get_fingerprint(zones::messages::CREATE_SURFACE).unwrap();
        assert_ne!(&result, create_fp);

        // But we can recover components by XOR-ing them out
        let col_fp = cb.get_fingerprint(zones::components::COLUMN).unwrap();
        let dark_fp = cb.get_fingerprint(zones::styles::THEME_DARK).unwrap();
        let recovered = &(&result ^ col_fp) ^ dark_fp;
        assert_eq!(&recovered, create_fp);
    }

    #[test]
    fn test_nearest_search() {
        let cb = Codebook::standard();
        let button_fp = cb.get_fingerprint(zones::components::BUTTON).unwrap();

        let (addr, dist) = cb.nearest(button_fp).unwrap();
        assert_eq!(addr, zones::components::BUTTON);
        assert_eq!(dist, 0);
    }

    #[test]
    fn test_codebook_diff_and_apply() {
        let cb1 = Codebook::standard();
        let mut cb2 = Codebook::standard();

        // Modify an entry in cb2
        let new_fp = HammingVector::from_seed("modified.button");
        cb2.entries.get_mut(&zones::components::BUTTON).unwrap().fingerprint = new_fp;
        cb2.version += 1;

        // Compute delta
        let delta = cb1.diff(&cb2);
        assert!(!delta.is_empty());

        // Apply delta to cb1
        let mut cb1_updated = cb1.clone();
        cb1_updated.apply_delta(&delta).unwrap();

        // Verify they match now
        let button1 = cb1_updated.get_fingerprint(zones::components::BUTTON).unwrap();
        let button2 = cb2.get_fingerprint(zones::components::BUTTON).unwrap();
        assert_eq!(button1, button2);
    }

    #[test]
    fn test_zone_entries() {
        let cb = Codebook::standard();
        let comp_entries = cb.zone_entries(&zones::ZONE_COMPONENTS);
        assert!(comp_entries.len() >= 20, "should have 20+ component entries");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let cb = Codebook::standard();
        let bytes = cb.to_bytes();
        let cb2 = Codebook::from_bytes(&bytes).unwrap();

        assert_eq!(cb.len(), cb2.len());
        assert_eq!(cb.version(), cb2.version());

        // Verify fingerprints match
        for (&addr, entry) in cb.iter() {
            let entry2 = cb2.get(addr).unwrap();
            assert_eq!(entry.fingerprint, entry2.fingerprint);
        }
    }
}
