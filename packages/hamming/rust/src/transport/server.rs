//! gRPC server implementation for A2UI Hamming transport.
//!
//! This server runs alongside the A2UI agent and translates
//! agent-generated UI operations into Hamming frames streamed
//! to the rendering client.

use crate::bitpack::PackedRef;
use crate::codebook::{Codebook, CodebookEntry};
use crate::frame::{FrameBuilder, HammingFrame, FramePayload, PayloadMode};
use crate::transport::{ServerState, Session};
use crate::vector::HammingVector;
use crate::xor_stream::{XorDeltaChunk, XorStream};
use crate::zones;

use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, Mutex};
use tokio_stream::wrappers::ReceiverStream;

/// A2UI Hamming transport server.
///
/// Bridges between A2UI agent logic and the Hamming protocol.
/// The agent calls methods on this server to encode UI operations
/// as Hamming frames, which are streamed to connected clients.
pub struct HammingServer {
    state: Arc<ServerState>,
}

impl HammingServer {
    /// Create a new server with the standard codebook.
    pub fn new(hmac_secret: Vec<u8>) -> Self {
        Self {
            state: Arc::new(ServerState::new(Codebook::standard(), hmac_secret)),
        }
    }

    /// Create with a custom codebook.
    pub fn with_codebook(codebook: Codebook, hmac_secret: Vec<u8>) -> Self {
        Self {
            state: Arc::new(ServerState::new(codebook, hmac_secret)),
        }
    }

    /// Get a reference to the server state.
    pub fn state(&self) -> &Arc<ServerState> {
        &self.state
    }

    /// Initialize a new client session.
    pub async fn init_session(
        &self,
        client_codebook_version: u32,
        udp_port: Option<u32>,
    ) -> SessionInitResult {
        let codebook = self.state.codebook.read().await;
        let server_version = codebook.version();

        // Derive session HMAC key
        let hmac_key = derive_session_key(&self.state.hmac_secret, server_version);

        // Create session
        let session_id = self
            .state
            .sessions
            .write()
            .await
            .create(hmac_key.clone(), server_version);

        // Determine if delta sync is possible or full sync needed
        let sync_mode = if client_codebook_version == 0 {
            SyncMode::FullSync
        } else if client_codebook_version == server_version {
            SyncMode::AlreadySynced
        } else {
            // Try to find delta path in XOR stream history
            let xor_stream = self.state.xor_stream.lock().await;
            match xor_stream.find_path(client_codebook_version, server_version) {
                Some(_path) => SyncMode::DeltaSync,
                None => SyncMode::FullSync,
            }
        };

        SessionInitResult {
            session_id,
            server_codebook_version: server_version,
            hmac_key,
            sync_mode,
        }
    }

    /// Encode a createSurface operation as Hamming frames.
    ///
    /// Maps the A2UI createSurface message to codebook references:
    /// - Message type (createSurface)
    /// - Root component type
    /// - Theme
    /// - Additional component references
    pub async fn encode_create_surface(
        &self,
        session_id: [u8; 8],
        surface_id: u32,
        root_component: u32,    // codebook address for root component type
        theme: Option<u32>,     // codebook address for theme
        additional: &[u32],     // additional codebook addresses
    ) -> crate::error::Result<HammingFrame> {
        let codebook = self.state.codebook.read().await;

        let mut builder = FrameBuilder::new(session_id, codebook.version())
            .surface(surface_id)
            .push_addr(zones::messages::CREATE_SURFACE as u16)
            .push_addr(root_component as u16);

        if let Some(theme_addr) = theme {
            builder = builder.push_addr(theme_addr as u16);
        }

        for &addr in additional {
            builder = builder.push_addr(addr as u16);
        }

        // Assign sequence number
        let mut sessions = self.state.sessions.write().await;
        if let Some(session) = sessions.get_mut(&session_id) {
            session.sequence_tx += 1;
            let frame = builder.sequence(session.sequence_tx).request_ack().build_refs();
            Ok(frame)
        } else {
            Err(crate::error::HammingError::Transport("session not found".into()))
        }
    }

    /// Encode an updateComponents operation.
    ///
    /// For simple updates (few components), uses Mode B (codebook refs).
    /// For complex compositions, uses Mode C (XOR chain).
    pub async fn encode_update_components(
        &self,
        session_id: [u8; 8],
        surface_id: u32,
        component_refs: &[(u32, u8)], // (address, modifier) pairs
    ) -> crate::error::Result<HammingFrame> {
        let codebook = self.state.codebook.read().await;

        let mut builder = FrameBuilder::new(session_id, codebook.version())
            .surface(surface_id)
            .push_addr(zones::messages::UPDATE_COMPONENTS as u16);

        for &(addr, modifier) in component_refs {
            builder = builder.push_ref(addr as u16, modifier);
        }

        let mut sessions = self.state.sessions.write().await;
        if let Some(session) = sessions.get_mut(&session_id) {
            session.sequence_tx += 1;
            Ok(builder.sequence(session.sequence_tx).build_refs())
        } else {
            Err(crate::error::HammingError::Transport("session not found".into()))
        }
    }

    /// Encode a user action (from client) into a Hamming frame.
    pub async fn encode_user_action(
        &self,
        session_id: [u8; 8],
        surface_id: u32,
        action_addr: u32,       // codebook address for the action type
        target_component: u32,  // codebook address for the target component
    ) -> crate::error::Result<HammingFrame> {
        let codebook = self.state.codebook.read().await;

        // Use XOR composition: action ⊕ target
        let frame = FrameBuilder::new(session_id, codebook.version())
            .surface(surface_id)
            .push_addr(action_addr as u16)
            .push_addr(target_component as u16)
            .build_xor();

        Ok(frame)
    }

    /// Encode a cognitive operation (routed to ladybug-rs).
    pub async fn encode_cognitive_op(
        &self,
        session_id: [u8; 8],
        op_addr: u32,
        query_fingerprint: Option<&HammingVector>,
    ) -> crate::error::Result<HammingFrame> {
        let codebook = self.state.codebook.read().await;

        match query_fingerprint {
            Some(fp) => {
                // Mode A: send the full fingerprint for semantic search
                let mut frame = HammingFrame::fingerprint(
                    session_id, 0, 0, codebook.version(), fp.clone(),
                );
                frame.header.flags.cognitive = true;
                Ok(frame)
            }
            None => {
                // Mode B: codebook ref for the cognitive operation
                let frame = FrameBuilder::new(session_id, codebook.version())
                    .push_addr(op_addr as u16)
                    .cognitive()
                    .build_refs();
                Ok(frame)
            }
        }
    }

    /// Stream codebook to client in chunks (for full sync).
    pub async fn stream_codebook(&self) -> Vec<XorDeltaChunk> {
        let codebook = self.state.codebook.read().await;
        let full_delta = XorStream::full_sync_delta(&codebook);
        XorStream::chunk_delta(&full_delta, 10) // 10 entries per chunk
    }

    /// Register a new dynamic codebook entry (learned from usage patterns).
    pub async fn register_dynamic_entry(
        &self,
        fingerprint: HammingVector,
        semantic_label: String,
        composed_from: Vec<u32>,
    ) -> crate::error::Result<u32> {
        let mut codebook = self.state.codebook.write().await;

        // Find next available address in dynamic zone (0xC00-0xEFF)
        let address = (0xC00..=0xEFF)
            .find(|&addr| codebook.get(addr).is_none())
            .ok_or(crate::error::HammingError::AddressOutOfRange {
                address: 0xEFF,
            })?;

        codebook.insert(CodebookEntry {
            address,
            fingerprint,
            zone: "dynamic",
            entry_version: 0,
            semantic_label,
            related: composed_from,
        })?;

        Ok(address)
    }
}

/// Result of session initialization.
pub struct SessionInitResult {
    pub session_id: [u8; 8],
    pub server_codebook_version: u32,
    pub hmac_key: Vec<u8>,
    pub sync_mode: SyncMode,
}

/// How to synchronize the codebook with the client.
pub enum SyncMode {
    /// Client has no codebook; send everything via Arrow Flight or chunked stream.
    FullSync,
    /// Client is behind; send XOR deltas.
    DeltaSync,
    /// Client is already in sync.
    AlreadySynced,
}

/// Derive a per-session HMAC key from the server secret and codebook version.
fn derive_session_key(secret: &[u8], codebook_version: u32) -> Vec<u8> {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher.update(b"a2ui-hamming-session-key");
    hasher.update(&codebook_version.to_le_bytes());
    hasher.update(&std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_le_bytes());
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_server_init_session() {
        let server = HammingServer::new(b"test-secret".to_vec());
        let result = server.init_session(0, None).await;

        assert_ne!(result.session_id, [0u8; 8]);
        assert!(result.server_codebook_version > 0);
        assert!(!result.hmac_key.is_empty());
        assert!(matches!(result.sync_mode, SyncMode::FullSync));
    }

    #[tokio::test]
    async fn test_encode_create_surface() {
        let server = HammingServer::new(b"test-secret".to_vec());
        let init = server.init_session(0, None).await;

        let frame = server
            .encode_create_surface(
                init.session_id,
                1,
                zones::components::COLUMN,
                Some(zones::styles::THEME_DARK),
                &[],
            )
            .await
            .unwrap();

        assert_eq!(frame.header.surface_id, 1);
        assert!(frame.header.flags.ack_requested);

        if let FramePayload::CodebookRefs(refs) = &frame.payload {
            assert_eq!(refs[0].address, zones::messages::CREATE_SURFACE as u16);
            assert_eq!(refs[1].address, zones::components::COLUMN as u16);
            assert_eq!(refs[2].address, zones::styles::THEME_DARK as u16);
        } else {
            panic!("expected CodebookRefs");
        }
    }

    #[tokio::test]
    async fn test_encode_user_action() {
        let server = HammingServer::new(b"test-secret".to_vec());
        let init = server.init_session(0, None).await;

        let frame = server
            .encode_user_action(
                init.session_id,
                1,
                zones::actions::CLICK,
                zones::components::BUTTON,
            )
            .await
            .unwrap();

        if let FramePayload::XorComposition { base, chain } = &frame.payload {
            assert_eq!(*base, zones::actions::CLICK as u16);
            assert_eq!(chain[0], zones::components::BUTTON as u16);
        } else {
            panic!("expected XorComposition");
        }
    }

    #[tokio::test]
    async fn test_register_dynamic_entry() {
        let server = HammingServer::new(b"test-secret".to_vec());

        let fp = HammingVector::from_seed("learned.card_with_button");
        let addr = server
            .register_dynamic_entry(
                fp,
                "card_with_button".into(),
                vec![zones::components::CARD, zones::components::BUTTON],
            )
            .await
            .unwrap();

        assert!(addr >= 0xC00 && addr <= 0xEFF);
    }

    #[tokio::test]
    async fn test_stream_codebook() {
        let server = HammingServer::new(b"test-secret".to_vec());
        let chunks = server.stream_codebook().await;

        assert!(!chunks.is_empty());
        assert!(chunks.last().unwrap().is_last);
    }
}
