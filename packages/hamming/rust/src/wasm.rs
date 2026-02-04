//! WASM bindings for the A2UI Hamming client.
//!
//! Exposes the Hamming protocol client to JavaScript/TypeScript via wasm-bindgen.
//! This runs in the browser alongside the Lit/Angular renderer.

#[cfg(feature = "wasm")]
mod bindings {
    use wasm_bindgen::prelude::*;
    use serde_wasm_bindgen;

    use crate::codebook::Codebook;
    use crate::frame::{FrameBuilder, HammingFrame};
    use crate::transport::client::{DecodedOperation, HammingClient};
    use crate::vector::HammingVector;
    use crate::zones;

    /// WASM-exposed Hamming protocol client.
    ///
    /// Usage from TypeScript:
    /// ```typescript
    /// import init, { WasmHammingClient } from 'a2ui-hamming';
    /// await init();
    /// const client = new WasmHammingClient(sessionId, hmacKey);
    /// const operation = client.decodeFrame(frameBytes);
    /// ```
    #[wasm_bindgen]
    pub struct WasmHammingClient {
        inner: HammingClient,
    }

    #[wasm_bindgen]
    impl WasmHammingClient {
        /// Create a new client with the standard codebook.
        #[wasm_bindgen(constructor)]
        pub fn new(session_id: &[u8], hmac_key: &[u8]) -> Result<WasmHammingClient, JsValue> {
            if session_id.len() != 8 {
                return Err(JsValue::from_str("session_id must be 8 bytes"));
            }
            let mut sid = [0u8; 8];
            sid.copy_from_slice(session_id);

            Ok(Self {
                inner: HammingClient::new(sid, hmac_key.to_vec()),
            })
        }

        /// Create a client with an empty codebook (needs full sync).
        #[wasm_bindgen(js_name = "empty")]
        pub fn empty(session_id: &[u8], hmac_key: &[u8]) -> Result<WasmHammingClient, JsValue> {
            if session_id.len() != 8 {
                return Err(JsValue::from_str("session_id must be 8 bytes"));
            }
            let mut sid = [0u8; 8];
            sid.copy_from_slice(session_id);

            Ok(Self {
                inner: HammingClient::empty(sid, hmac_key.to_vec()),
            })
        }

        /// Get the local codebook version.
        #[wasm_bindgen(js_name = "codebookVersion")]
        pub fn codebook_version(&self) -> u32 {
            self.inner.codebook_version()
        }

        /// Decode a raw frame (Uint8Array) into a JS object describing the operation.
        ///
        /// Returns a JSON-serializable object with the decoded operation.
        #[wasm_bindgen(js_name = "decodeFrame")]
        pub fn decode_frame(&mut self, data: &[u8]) -> Result<JsValue, JsValue> {
            let op = self.inner.decode_frame(data).map_err(|e| JsValue::from_str(&e.to_string()))?;
            serde_wasm_bindgen::to_value(&WasmDecodedOp::from(op))
                .map_err(|e| JsValue::from_str(&e.to_string()))
        }

        /// Look up a codebook entry by address. Returns the semantic label.
        #[wasm_bindgen(js_name = "lookupAddress")]
        pub fn lookup_address(&self, address: u32) -> Option<String> {
            self.inner.codebook().get(address).map(|e| e.semantic_label.clone())
        }

        /// Get the Hamming distance between two codebook entries.
        #[wasm_bindgen(js_name = "hammingDistance")]
        pub fn hamming_distance(&self, addr_a: u32, addr_b: u32) -> Option<u32> {
            let fp_a = self.inner.codebook().get_fingerprint(addr_a)?;
            let fp_b = self.inner.codebook().get_fingerprint(addr_b)?;
            Some(fp_a.hamming_distance(fp_b))
        }

        /// Generate a fingerprint from a string seed (for custom entries).
        #[wasm_bindgen(js_name = "fingerprint")]
        pub fn fingerprint(seed: &str) -> Vec<u8> {
            HammingVector::from_seed(seed).to_bytes()
        }

        /// Build a codebook reference frame (Mode B) from an array of addresses.
        ///
        /// Returns the encoded frame as Uint8Array.
        #[wasm_bindgen(js_name = "buildRefFrame")]
        pub fn build_ref_frame(
            &self,
            surface_id: u32,
            sequence: u32,
            addresses: &[u16],
            hmac_key: &[u8],
        ) -> Result<Vec<u8>, JsValue> {
            let session_id = self.inner.codebook().version(); // use version as part of session
            let mut sid = [0u8; 8];
            sid[..4].copy_from_slice(&session_id.to_le_bytes());

            let mut builder = FrameBuilder::new(sid, self.inner.codebook_version())
                .sequence(sequence)
                .surface(surface_id);

            for &addr in addresses {
                builder = builder.push_addr(addr);
            }

            let frame = builder.build_refs();
            frame
                .encode(hmac_key)
                .map_err(|e| JsValue::from_str(&e.to_string()))
        }

        /// Build an XOR composition frame (Mode C).
        #[wasm_bindgen(js_name = "buildXorFrame")]
        pub fn build_xor_frame(
            &self,
            surface_id: u32,
            sequence: u32,
            base: u16,
            chain: &[u16],
            hmac_key: &[u8],
        ) -> Result<Vec<u8>, JsValue> {
            let mut sid = [0u8; 8];
            let v = self.inner.codebook_version();
            sid[..4].copy_from_slice(&v.to_le_bytes());

            let frame = HammingFrame::xor_composition(sid, sequence, surface_id, v, base, chain.to_vec());
            frame
                .encode(hmac_key)
                .map_err(|e| JsValue::from_str(&e.to_string()))
        }

        /// Get codebook statistics.
        #[wasm_bindgen(js_name = "codebookStats")]
        pub fn codebook_stats(&self) -> JsValue {
            let cb = self.inner.codebook();
            let stats = WasmCodebookStats {
                version: cb.version(),
                entry_count: cb.len() as u32,
                zones: crate::zones::ALL_ZONES
                    .iter()
                    .map(|z| WasmZoneStats {
                        name: z.name.to_string(),
                        capacity: z.capacity(),
                        used: cb.zone_entries(z).len() as u32,
                    })
                    .collect(),
            };
            serde_wasm_bindgen::to_value(&stats).unwrap_or(JsValue::NULL)
        }
    }

    // ── Serializable types for WASM boundary ──────────────

    #[derive(serde::Serialize)]
    struct WasmDecodedOp {
        op_type: String,
        surface_id: u32,
        details: serde_json::Value,
    }

    impl From<DecodedOperation> for WasmDecodedOp {
        fn from(op: DecodedOperation) -> Self {
            match op {
                DecodedOperation::CreateSurface {
                    surface_id,
                    root_component,
                    theme,
                    additional,
                } => Self {
                    op_type: "createSurface".into(),
                    surface_id,
                    details: serde_json::json!({
                        "rootComponent": {
                            "type": root_component.component_type,
                            "address": format!("0x{:03X}", root_component.address),
                        },
                        "theme": theme.map(|t| serde_json::json!({
                            "type": t.style_type,
                            "address": format!("0x{:03X}", t.address),
                        })),
                        "additionalRefs": additional.len(),
                    }),
                },
                DecodedOperation::UpdateComponents {
                    surface_id,
                    components,
                } => Self {
                    op_type: "updateComponents".into(),
                    surface_id,
                    details: serde_json::json!({
                        "components": components.iter().map(|c| serde_json::json!({
                            "label": c.semantic_label,
                            "address": format!("0x{:03X}", c.address),
                            "zone": c.zone,
                        })).collect::<Vec<_>>(),
                    }),
                },
                DecodedOperation::UpdateDataModel {
                    surface_id,
                    data_refs,
                } => Self {
                    op_type: "updateDataModel".into(),
                    surface_id,
                    details: serde_json::json!({
                        "refs": data_refs.len(),
                    }),
                },
                DecodedOperation::DeleteSurface { surface_id } => Self {
                    op_type: "deleteSurface".into(),
                    surface_id,
                    details: serde_json::json!({}),
                },
                DecodedOperation::UserAction {
                    surface_id,
                    action,
                } => Self {
                    op_type: "userAction".into(),
                    surface_id,
                    details: serde_json::json!({
                        "actionType": action.action_type,
                        "targetComponent": action.target_component,
                    }),
                },
                DecodedOperation::CognitiveOp { op_refs, .. } => Self {
                    op_type: "cognitiveOp".into(),
                    surface_id: 0,
                    details: serde_json::json!({
                        "refs": op_refs.len(),
                    }),
                },
                DecodedOperation::Raw {
                    surface_id,
                    refs,
                } => Self {
                    op_type: "raw".into(),
                    surface_id,
                    details: serde_json::json!({
                        "refs": refs.iter().map(|r| serde_json::json!({
                            "label": r.semantic_label,
                            "address": format!("0x{:03X}", r.address),
                            "zone": r.zone,
                        })).collect::<Vec<_>>(),
                    }),
                },
            }
        }
    }

    #[derive(serde::Serialize)]
    struct WasmCodebookStats {
        version: u32,
        entry_count: u32,
        zones: Vec<WasmZoneStats>,
    }

    #[derive(serde::Serialize)]
    struct WasmZoneStats {
        name: String,
        capacity: u32,
        used: u32,
    }
}
