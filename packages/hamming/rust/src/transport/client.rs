//! gRPC client implementation for A2UI Hamming transport.
//!
//! Runs on the rendering client side. Receives Hamming frames,
//! resolves codebook references, and produces decoded A2UI operations
//! that the renderer (Lit/Angular/WASM) can apply to surfaces.

use crate::bitpack::PackedRef;
use crate::codebook::Codebook;
use crate::error::{HammingError, Result};
use crate::frame::{FramePayload, HammingFrame, PayloadMode};
use crate::vector::HammingVector;
use crate::xor_stream::{DeltaAccumulator, XorDelta, XorStream};
use crate::zones;

/// Decoded A2UI operation from a Hamming frame.
///
/// This is the output of the Hamming client — the renderer consumes
/// these operations to build/update the UI. No JSON involved.
#[derive(Debug, Clone)]
pub enum DecodedOperation {
    /// Create a new surface with the given properties
    CreateSurface {
        surface_id: u32,
        root_component: ResolvedComponent,
        theme: Option<ResolvedStyle>,
        additional: Vec<ResolvedRef>,
    },
    /// Update components on a surface
    UpdateComponents {
        surface_id: u32,
        components: Vec<ResolvedRef>,
    },
    /// Update the data model
    UpdateDataModel {
        surface_id: u32,
        data_refs: Vec<ResolvedRef>,
    },
    /// Delete a surface
    DeleteSurface {
        surface_id: u32,
    },
    /// A user action (decoded from XOR composition)
    UserAction {
        surface_id: u32,
        action: ResolvedAction,
    },
    /// A cognitive operation (for ladybug-rs routing)
    CognitiveOp {
        fingerprint: Option<HammingVector>,
        op_refs: Vec<ResolvedRef>,
    },
    /// Raw resolved references (when message type is unknown)
    Raw {
        surface_id: u32,
        refs: Vec<ResolvedRef>,
    },
}

/// A resolved codebook reference with semantic information.
#[derive(Debug, Clone)]
pub struct ResolvedRef {
    pub address: u32,
    pub modifier: u8,
    pub zone: String,
    pub semantic_label: String,
    pub fingerprint: HammingVector,
}

/// A resolved component reference.
#[derive(Debug, Clone)]
pub struct ResolvedComponent {
    pub address: u32,
    pub component_type: String,
    pub modifier: u8,
}

/// A resolved style reference.
#[derive(Debug, Clone)]
pub struct ResolvedStyle {
    pub address: u32,
    pub style_type: String,
}

/// A resolved user action.
#[derive(Debug, Clone)]
pub struct ResolvedAction {
    pub action_type: String,
    pub target_component: Option<String>,
    pub composed_fingerprint: HammingVector,
}

/// The Hamming protocol client.
///
/// Maintains a local codebook, receives and decodes frames, and outputs
/// decoded A2UI operations for the renderer.
pub struct HammingClient {
    /// Local codebook (synchronized with server)
    codebook: Codebook,
    /// HMAC key for frame verification
    hmac_key: Vec<u8>,
    /// Session identifier
    session_id: [u8; 8],
    /// Last received sequence number
    last_sequence: u32,
    /// Error correction threshold (max Hamming distance for auto-correction)
    error_threshold: u32,
    /// Codebook delta accumulator (for chunked sync)
    delta_accumulator: Option<DeltaAccumulator>,
}

impl HammingClient {
    /// Create a new client with the standard codebook.
    pub fn new(session_id: [u8; 8], hmac_key: Vec<u8>) -> Self {
        Self {
            codebook: Codebook::standard(),
            hmac_key,
            session_id,
            last_sequence: 0,
            error_threshold: 50,
            delta_accumulator: None,
        }
    }

    /// Create with an empty codebook (requires full sync before use).
    pub fn empty(session_id: [u8; 8], hmac_key: Vec<u8>) -> Self {
        Self {
            codebook: Codebook::new(),
            hmac_key,
            session_id,
            last_sequence: 0,
            error_threshold: 50,
            delta_accumulator: None,
        }
    }

    /// Get a reference to the local codebook.
    pub fn codebook(&self) -> &Codebook {
        &self.codebook
    }

    /// Get the codebook version.
    pub fn codebook_version(&self) -> u32 {
        self.codebook.version()
    }

    /// Apply a codebook delta (from XOR stream sync).
    pub fn apply_delta(&mut self, delta: &XorDelta) -> Result<()> {
        let stream = XorStream::new(0);
        stream.apply_delta(&mut self.codebook, delta)
    }

    /// Decode a raw UDP frame into an A2UI operation.
    pub fn decode_frame(&mut self, data: &[u8]) -> Result<DecodedOperation> {
        let frame = HammingFrame::decode(data, &self.hmac_key)?;
        self.process_frame(frame)
    }

    /// Process a decoded frame into an A2UI operation.
    pub fn process_frame(&mut self, frame: HammingFrame) -> Result<DecodedOperation> {
        // Track sequence
        self.last_sequence = frame.header.sequence;

        let surface_id = frame.header.surface_id;

        // Route cognitive frames separately
        if frame.header.flags.cognitive {
            return self.decode_cognitive(frame);
        }

        match frame.payload {
            FramePayload::Fingerprint(fp) => {
                // Mode A: novel fingerprint, try to find nearest match
                self.decode_fingerprint(surface_id, fp)
            }
            FramePayload::CodebookRefs(refs) => {
                // Mode B: resolve codebook references
                self.decode_refs(surface_id, &refs)
            }
            FramePayload::XorComposition { base, chain } => {
                // Mode C: resolve XOR composition
                self.decode_xor(surface_id, base, &chain)
            }
            FramePayload::Mixed(entries) => {
                // Mode D: mixed entries
                self.decode_mixed(surface_id, entries)
            }
        }
    }

    /// Decode Mode A (single fingerprint).
    fn decode_fingerprint(
        &self,
        surface_id: u32,
        fp: HammingVector,
    ) -> Result<DecodedOperation> {
        // Find nearest codebook entry
        match self.codebook.nearest(&fp) {
            Some((addr, distance)) if distance <= self.error_threshold => {
                // Close enough — treat as that entry (error correction)
                let entry = self.codebook.get(addr).unwrap();
                let resolved = self.resolve_entry(addr, 0);
                Ok(DecodedOperation::Raw {
                    surface_id,
                    refs: vec![resolved],
                })
            }
            _ => {
                // Truly novel — return as raw fingerprint
                Ok(DecodedOperation::Raw {
                    surface_id,
                    refs: vec![ResolvedRef {
                        address: 0xFFFF,
                        modifier: 0,
                        zone: "unknown".into(),
                        semantic_label: "novel_fingerprint".into(),
                        fingerprint: fp,
                    }],
                })
            }
        }
    }

    /// Decode Mode B (codebook references).
    fn decode_refs(
        &self,
        surface_id: u32,
        refs: &[PackedRef],
    ) -> Result<DecodedOperation> {
        if refs.is_empty() {
            return Ok(DecodedOperation::Raw {
                surface_id,
                refs: Vec::new(),
            });
        }

        // First ref determines the message type
        let msg_type_addr = refs[0].address as u32;

        match msg_type_addr {
            addr if addr == zones::messages::CREATE_SURFACE => {
                let root_component = refs
                    .get(1)
                    .map(|r| self.resolve_component(r.address as u32, r.modifier))
                    .unwrap_or(ResolvedComponent {
                        address: zones::components::COLUMN,
                        component_type: "Column".into(),
                        modifier: 0,
                    });

                let theme = refs.iter().skip(2).find_map(|r| {
                    let addr = r.address as u32;
                    if addr >= 0x400 && addr <= 0x5FF {
                        Some(self.resolve_style(addr))
                    } else {
                        None
                    }
                });

                let additional: Vec<ResolvedRef> = refs
                    .iter()
                    .skip(2)
                    .filter(|r| {
                        let addr = r.address as u32;
                        !(addr >= 0x400 && addr <= 0x5FF) // skip already-extracted styles
                    })
                    .map(|r| self.resolve_entry(r.address as u32, r.modifier))
                    .collect();

                Ok(DecodedOperation::CreateSurface {
                    surface_id,
                    root_component,
                    theme,
                    additional,
                })
            }
            addr if addr == zones::messages::UPDATE_COMPONENTS => {
                let components: Vec<ResolvedRef> = refs
                    .iter()
                    .skip(1)
                    .map(|r| self.resolve_entry(r.address as u32, r.modifier))
                    .collect();

                Ok(DecodedOperation::UpdateComponents {
                    surface_id,
                    components,
                })
            }
            addr if addr == zones::messages::UPDATE_DATA_MODEL => {
                let data_refs: Vec<ResolvedRef> = refs
                    .iter()
                    .skip(1)
                    .map(|r| self.resolve_entry(r.address as u32, r.modifier))
                    .collect();

                Ok(DecodedOperation::UpdateDataModel {
                    surface_id,
                    data_refs,
                })
            }
            addr if addr == zones::messages::DELETE_SURFACE => {
                Ok(DecodedOperation::DeleteSurface { surface_id })
            }
            _ => {
                // Unknown message type — return raw
                let all_refs: Vec<ResolvedRef> = refs
                    .iter()
                    .map(|r| self.resolve_entry(r.address as u32, r.modifier))
                    .collect();

                Ok(DecodedOperation::Raw {
                    surface_id,
                    refs: all_refs,
                })
            }
        }
    }

    /// Decode Mode C (XOR composition).
    fn decode_xor(
        &self,
        surface_id: u32,
        base: u16,
        chain: &[u16],
    ) -> Result<DecodedOperation> {
        let base_addr = base as u32;

        // Check if this is an action (base in action zone)
        if base_addr >= 0x800 && base_addr <= 0x9FF {
            let action_type = self
                .codebook
                .get(base_addr)
                .map(|e| e.semantic_label.clone())
                .unwrap_or_else(|| format!("action_0x{:03X}", base_addr));

            let target_component = chain.first().and_then(|&addr| {
                self.codebook
                    .get(addr as u32)
                    .map(|e| e.semantic_label.clone())
            });

            // Compute the composed fingerprint
            let mut addrs: Vec<u32> = vec![base_addr];
            addrs.extend(chain.iter().map(|&a| a as u32));
            let composed = self.codebook.resolve_xor_chain(&addrs)?;

            return Ok(DecodedOperation::UserAction {
                surface_id,
                action: ResolvedAction {
                    action_type,
                    target_component,
                    composed_fingerprint: composed,
                },
            });
        }

        // General XOR composition — resolve and return as raw
        let mut addrs: Vec<u32> = vec![base_addr];
        addrs.extend(chain.iter().map(|&a| a as u32));
        let composed = self.codebook.resolve_xor_chain(&addrs)?;

        // Try to find what the composition matches
        let resolved = match self.codebook.nearest(&composed) {
            Some((addr, distance)) if distance <= self.error_threshold => {
                vec![self.resolve_entry(addr, 0)]
            }
            _ => {
                // Return individual refs
                addrs
                    .iter()
                    .map(|&a| self.resolve_entry(a, 0))
                    .collect()
            }
        };

        Ok(DecodedOperation::Raw {
            surface_id,
            refs: resolved,
        })
    }

    /// Decode mixed payload.
    fn decode_mixed(
        &self,
        surface_id: u32,
        entries: Vec<crate::frame::MixedEntry>,
    ) -> Result<DecodedOperation> {
        let refs: Vec<ResolvedRef> = entries
            .into_iter()
            .filter_map(|e| match e {
                crate::frame::MixedEntry::Ref(r) => {
                    Some(self.resolve_entry(r.address as u32, r.modifier))
                }
                crate::frame::MixedEntry::InlineFingerprint(fp) => {
                    match self.codebook.nearest(&fp) {
                        Some((addr, dist)) if dist <= self.error_threshold => {
                            Some(self.resolve_entry(addr, 0))
                        }
                        _ => Some(ResolvedRef {
                            address: 0xFFFF,
                            modifier: 0,
                            zone: "inline".into(),
                            semantic_label: "inline_fingerprint".into(),
                            fingerprint: fp,
                        }),
                    }
                }
                crate::frame::MixedEntry::InlineData(_) => None,
            })
            .collect();

        Ok(DecodedOperation::Raw {
            surface_id,
            refs,
        })
    }

    /// Decode cognitive frame.
    fn decode_cognitive(&self, frame: HammingFrame) -> Result<DecodedOperation> {
        match frame.payload {
            FramePayload::Fingerprint(fp) => Ok(DecodedOperation::CognitiveOp {
                fingerprint: Some(fp),
                op_refs: Vec::new(),
            }),
            FramePayload::CodebookRefs(refs) => {
                let resolved: Vec<ResolvedRef> = refs
                    .iter()
                    .map(|r| self.resolve_entry(r.address as u32, r.modifier))
                    .collect();
                Ok(DecodedOperation::CognitiveOp {
                    fingerprint: None,
                    op_refs: resolved,
                })
            }
            _ => Ok(DecodedOperation::CognitiveOp {
                fingerprint: None,
                op_refs: Vec::new(),
            }),
        }
    }

    // ── Resolution helpers ──────────────────────────────

    fn resolve_entry(&self, address: u32, modifier: u8) -> ResolvedRef {
        match self.codebook.get(address) {
            Some(entry) => ResolvedRef {
                address,
                modifier,
                zone: entry.zone.to_string(),
                semantic_label: entry.semantic_label.clone(),
                fingerprint: entry.fingerprint.clone(),
            },
            None => ResolvedRef {
                address,
                modifier,
                zone: "unknown".into(),
                semantic_label: format!("unknown_0x{:03X}", address),
                fingerprint: HammingVector::zero(),
            },
        }
    }

    fn resolve_component(&self, address: u32, modifier: u8) -> ResolvedComponent {
        let component_type = self
            .codebook
            .get(address)
            .map(|e| {
                e.semantic_label
                    .strip_prefix("a2ui.component.")
                    .unwrap_or(&e.semantic_label)
                    .to_string()
            })
            .unwrap_or_else(|| format!("Component_0x{:03X}", address));

        ResolvedComponent {
            address,
            component_type,
            modifier,
        }
    }

    fn resolve_style(&self, address: u32) -> ResolvedStyle {
        let style_type = self
            .codebook
            .get(address)
            .map(|e| {
                e.semantic_label
                    .strip_prefix("a2ui.style.")
                    .unwrap_or(&e.semantic_label)
                    .to_string()
            })
            .unwrap_or_else(|| format!("Style_0x{:03X}", address));

        ResolvedStyle {
            address,
            style_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FrameBuilder;

    fn test_client() -> HammingClient {
        HammingClient::new([1, 2, 3, 4, 5, 6, 7, 8], b"test-key".to_vec())
    }

    #[test]
    fn test_decode_create_surface() {
        let client = test_client();
        let frame = FrameBuilder::new(client.session_id, 1)
            .surface(1)
            .push_addr(zones::messages::CREATE_SURFACE as u16)
            .push_addr(zones::components::COLUMN as u16)
            .push_addr(zones::styles::THEME_DARK as u16)
            .build_refs();

        let op = client.process_frame(frame).unwrap();

        if let DecodedOperation::CreateSurface {
            surface_id,
            root_component,
            theme,
            ..
        } = op
        {
            assert_eq!(surface_id, 1);
            assert_eq!(root_component.component_type, "Column");
            assert!(theme.is_some());
            assert_eq!(theme.unwrap().style_type, "theme.dark");
        } else {
            panic!("expected CreateSurface, got {:?}", op);
        }
    }

    #[test]
    fn test_decode_update_components() {
        let client = test_client();
        let frame = FrameBuilder::new(client.session_id, 1)
            .surface(1)
            .push_addr(zones::messages::UPDATE_COMPONENTS as u16)
            .push_addr(zones::components::TEXT as u16)
            .push_addr(zones::components::BUTTON as u16)
            .push_addr(zones::components::IMAGE as u16)
            .build_refs();

        let op = client.process_frame(frame).unwrap();

        if let DecodedOperation::UpdateComponents { components, .. } = op {
            assert_eq!(components.len(), 3);
            assert!(components[0].semantic_label.contains("Text"));
            assert!(components[1].semantic_label.contains("Button"));
            assert!(components[2].semantic_label.contains("Image"));
        } else {
            panic!("expected UpdateComponents");
        }
    }

    #[test]
    fn test_decode_user_action() {
        let client = test_client();
        let frame = FrameBuilder::new(client.session_id, 1)
            .surface(1)
            .push_addr(zones::actions::CLICK as u16)
            .push_addr(zones::components::BUTTON as u16)
            .build_xor();

        let op = client.process_frame(frame).unwrap();

        if let DecodedOperation::UserAction { action, .. } = op {
            assert!(action.action_type.contains("click"));
            assert!(action.target_component.unwrap().contains("Button"));
        } else {
            panic!("expected UserAction");
        }
    }

    #[test]
    fn test_decode_delete_surface() {
        let client = test_client();
        let frame = FrameBuilder::new(client.session_id, 1)
            .surface(42)
            .push_addr(zones::messages::DELETE_SURFACE as u16)
            .build_refs();

        let op = client.process_frame(frame).unwrap();

        if let DecodedOperation::DeleteSurface { surface_id } = op {
            assert_eq!(surface_id, 42);
        } else {
            panic!("expected DeleteSurface");
        }
    }

    #[test]
    fn test_decode_cognitive() {
        let client = test_client();
        let frame = FrameBuilder::new(client.session_id, 1)
            .push_addr(zones::cognitive::SEMANTIC_SEARCH as u16)
            .cognitive()
            .build_refs();

        let op = client.process_frame(frame).unwrap();
        assert!(matches!(op, DecodedOperation::CognitiveOp { .. }));
    }
}
