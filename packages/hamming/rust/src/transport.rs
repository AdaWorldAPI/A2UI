//! gRPC transport layer for A2UI Hamming protocol.
//!
//! Provides both server and client implementations using tonic.
//! The server bridges between A2UI agent output and the Hamming protocol;
//! the client runs in the renderer and decodes frames into A2UI operations.
//!
//! For bulk codebook sync, integrates with Arrow Flight via ladybug-rs.

pub mod server;
pub mod client;

use crate::codebook::Codebook;
use crate::frame::HammingFrame;
use crate::vector::HammingVector;
use crate::xor_stream::XorStream;

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Shared state for the Hamming transport server.
pub struct ServerState {
    /// The authoritative codebook
    pub codebook: Arc<RwLock<Codebook>>,
    /// XOR stream manager for delta distribution
    pub xor_stream: Arc<Mutex<XorStream>>,
    /// Active sessions
    pub sessions: Arc<RwLock<SessionStore>>,
    /// HMAC key derivation secret
    pub hmac_secret: Vec<u8>,
}

impl ServerState {
    pub fn new(codebook: Codebook, hmac_secret: Vec<u8>) -> Self {
        Self {
            codebook: Arc::new(RwLock::new(codebook)),
            xor_stream: Arc::new(Mutex::new(XorStream::new(1000))),
            sessions: Arc::new(RwLock::new(SessionStore::new())),
            hmac_secret,
        }
    }
}

/// Session tracking.
pub struct Session {
    pub id: [u8; 8],
    pub codebook_version: u32,
    pub hmac_key: Vec<u8>,
    pub sequence_tx: u32,
    pub sequence_rx: u32,
    pub udp_endpoint: Option<String>,
}

/// In-memory session store.
pub struct SessionStore {
    sessions: std::collections::HashMap<[u8; 8], Session>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: std::collections::HashMap::new(),
        }
    }

    pub fn create(&mut self, hmac_key: Vec<u8>, codebook_version: u32) -> [u8; 8] {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(&(self.sessions.len() as u64).to_le_bytes());
        hasher.update(&std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes());
        let hash = hasher.finalize();
        let mut id = [0u8; 8];
        id.copy_from_slice(&hash[..8]);

        self.sessions.insert(
            id,
            Session {
                id,
                codebook_version,
                hmac_key,
                sequence_tx: 0,
                sequence_rx: 0,
                udp_endpoint: None,
            },
        );

        id
    }

    pub fn get(&self, id: &[u8; 8]) -> Option<&Session> {
        self.sessions.get(id)
    }

    pub fn get_mut(&mut self, id: &[u8; 8]) -> Option<&mut Session> {
        self.sessions.get_mut(id)
    }

    pub fn remove(&mut self, id: &[u8; 8]) -> Option<Session> {
        self.sessions.remove(id)
    }
}
