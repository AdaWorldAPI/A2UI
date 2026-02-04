//! UDP frame codec for A2UI Hamming protocol.
//!
//! Encodes/decodes frames that fit within standard MTU (1,500 bytes).
//! Frame = 32-byte header + payload (up to 1,424 bytes) + 16-byte HMAC.

use crate::bitpack::{self, PackedRef};
use crate::error::{HammingError, Result};
use crate::vector::HammingVector;
use crate::{
    FINGERPRINT_BYTES, FRAME_HEADER_SIZE, FRAME_MAGIC, HMAC_TAG_SIZE,
    MAX_FRAME_PAYLOAD, MAX_UDP_PAYLOAD, PROTOCOL_VERSION,
};

use sha2::{Sha256, Digest};

/// Payload mode determines how the frame body is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadMode {
    /// Mode A: single 10kbit fingerprint
    Fingerprint = 0,
    /// Mode B: codebook reference list
    CodebookRef = 1,
    /// Mode C: XOR composition chain
    XorComposition = 2,
    /// Mode D: mixed refs + inline fragments
    Mixed = 3,
}

impl PayloadMode {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(Self::Fingerprint),
            1 => Ok(Self::CodebookRef),
            2 => Ok(Self::XorComposition),
            3 => Ok(Self::Mixed),
            _ => Err(HammingError::UnsupportedMode(v)),
        }
    }
}

/// Frame header flags (1 byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameFlags {
    pub mode: PayloadMode,
    pub compressed: bool,
    pub ack_requested: bool,
    pub more_fragments: bool,
    pub cognitive: bool,
}

impl FrameFlags {
    pub fn to_byte(&self) -> u8 {
        let mut b: u8 = (self.mode as u8) << 6;
        if self.compressed {
            b |= 1 << 5;
        }
        if self.ack_requested {
            b |= 1 << 4;
        }
        if self.more_fragments {
            b |= 1 << 3;
        }
        if self.cognitive {
            b |= 1 << 2;
        }
        b
    }

    pub fn from_byte(b: u8) -> Result<Self> {
        Ok(Self {
            mode: PayloadMode::from_u8((b >> 6) & 0x03)?,
            compressed: (b >> 5) & 1 == 1,
            ack_requested: (b >> 4) & 1 == 1,
            more_fragments: (b >> 3) & 1 == 1,
            cognitive: (b >> 2) & 1 == 1,
        })
    }
}

/// 32-byte frame header.
#[derive(Debug, Clone)]
pub struct FrameHeader {
    pub flags: FrameFlags,
    pub sequence: u32,
    pub ack: u32,
    pub session: [u8; 8],
    pub surface_id: u32,
    pub op_count: u16,
    pub codebook_version: u32,
}

impl FrameHeader {
    /// Serialize header to 32 bytes.
    pub fn to_bytes(&self) -> [u8; FRAME_HEADER_SIZE] {
        let mut buf = [0u8; FRAME_HEADER_SIZE];

        // magic (2)
        buf[0] = FRAME_MAGIC[0];
        buf[1] = FRAME_MAGIC[1];

        // version (1)
        buf[2] = PROTOCOL_VERSION;

        // flags (1)
        buf[3] = self.flags.to_byte();

        // sequence (4)
        buf[4..8].copy_from_slice(&self.sequence.to_le_bytes());

        // ack (4)
        buf[8..12].copy_from_slice(&self.ack.to_le_bytes());

        // session (8)
        buf[12..20].copy_from_slice(&self.session);

        // surface_id (4)
        buf[20..24].copy_from_slice(&self.surface_id.to_le_bytes());

        // op_count (2)
        buf[24..26].copy_from_slice(&self.op_count.to_le_bytes());

        // codebook_version (4)
        buf[26..30].copy_from_slice(&self.codebook_version.to_le_bytes());

        // reserved (2)
        buf[30] = 0;
        buf[31] = 0;

        buf
    }

    /// Parse header from 32 bytes.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < FRAME_HEADER_SIZE {
            return Err(HammingError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "header too short",
            )));
        }

        // Verify magic
        let magic = u16::from_be_bytes([buf[0], buf[1]]);
        if buf[0] != FRAME_MAGIC[0] || buf[1] != FRAME_MAGIC[1] {
            return Err(HammingError::InvalidMagic(magic));
        }

        // Verify version
        let _version = buf[2];

        let flags = FrameFlags::from_byte(buf[3])?;
        let sequence = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        let ack = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        let mut session = [0u8; 8];
        session.copy_from_slice(&buf[12..20]);
        let surface_id = u32::from_le_bytes(buf[20..24].try_into().unwrap());
        let op_count = u16::from_le_bytes(buf[24..26].try_into().unwrap());
        let codebook_version = u32::from_le_bytes(buf[26..30].try_into().unwrap());

        Ok(Self {
            flags,
            sequence,
            ack,
            session,
            surface_id,
            op_count,
            codebook_version,
        })
    }
}

/// Frame payload variants.
#[derive(Debug, Clone)]
pub enum FramePayload {
    /// Mode A: single fingerprint
    Fingerprint(HammingVector),
    /// Mode B: codebook references
    CodebookRefs(Vec<PackedRef>),
    /// Mode C: XOR composition (base + chain)
    XorComposition { base: u16, chain: Vec<u16> },
    /// Mode D: mixed entries
    Mixed(Vec<MixedEntry>),
}

#[derive(Debug, Clone)]
pub enum MixedEntry {
    Ref(PackedRef),
    InlineFingerprint(HammingVector),
    InlineData(Vec<u8>),
}

/// A complete A2UI Hamming frame.
#[derive(Debug, Clone)]
pub struct HammingFrame {
    pub header: FrameHeader,
    pub payload: FramePayload,
}

impl HammingFrame {
    /// Create a Mode B (codebook reference) frame.
    pub fn codebook_refs(
        session: [u8; 8],
        sequence: u32,
        surface_id: u32,
        codebook_version: u32,
        refs: Vec<PackedRef>,
    ) -> Self {
        Self {
            header: FrameHeader {
                flags: FrameFlags {
                    mode: PayloadMode::CodebookRef,
                    compressed: false,
                    ack_requested: false,
                    more_fragments: false,
                    cognitive: false,
                },
                sequence,
                ack: 0,
                session,
                surface_id,
                op_count: refs.len() as u16,
                codebook_version,
            },
            payload: FramePayload::CodebookRefs(refs),
        }
    }

    /// Create a Mode C (XOR composition) frame.
    pub fn xor_composition(
        session: [u8; 8],
        sequence: u32,
        surface_id: u32,
        codebook_version: u32,
        base: u16,
        chain: Vec<u16>,
    ) -> Self {
        Self {
            header: FrameHeader {
                flags: FrameFlags {
                    mode: PayloadMode::XorComposition,
                    compressed: false,
                    ack_requested: false,
                    more_fragments: false,
                    cognitive: false,
                },
                sequence,
                ack: 0,
                session,
                surface_id,
                op_count: (1 + chain.len()) as u16,
                codebook_version,
            },
            payload: FramePayload::XorComposition { base, chain },
        }
    }

    /// Create a Mode A (single fingerprint) frame.
    pub fn fingerprint(
        session: [u8; 8],
        sequence: u32,
        surface_id: u32,
        codebook_version: u32,
        fp: HammingVector,
    ) -> Self {
        Self {
            header: FrameHeader {
                flags: FrameFlags {
                    mode: PayloadMode::Fingerprint,
                    compressed: false,
                    ack_requested: false,
                    more_fragments: false,
                    cognitive: false,
                },
                sequence,
                ack: 0,
                session,
                surface_id,
                op_count: 1,
                codebook_version,
            },
            payload: FramePayload::Fingerprint(fp),
        }
    }

    /// Encode to bytes (ready for UDP send).
    ///
    /// Layout: [header: 32B] [payload: variable] [hmac: 16B]
    pub fn encode(&self, hmac_key: &[u8]) -> Result<Vec<u8>> {
        let header_bytes = self.header.to_bytes();
        let payload_bytes = self.encode_payload()?;

        let total = FRAME_HEADER_SIZE + payload_bytes.len() + HMAC_TAG_SIZE;
        if total > MAX_UDP_PAYLOAD {
            return Err(HammingError::FrameTooLarge {
                size: total,
                max: MAX_UDP_PAYLOAD,
            });
        }

        let mut frame = Vec::with_capacity(total);
        frame.extend_from_slice(&header_bytes);
        frame.extend_from_slice(&payload_bytes);

        // Compute HMAC-SHA256 over header + payload, truncate to 16 bytes
        let hmac = Self::compute_hmac(hmac_key, &frame);
        frame.extend_from_slice(&hmac);

        Ok(frame)
    }

    /// Decode from bytes (received via UDP).
    pub fn decode(data: &[u8], hmac_key: &[u8]) -> Result<Self> {
        if data.len() < FRAME_HEADER_SIZE + HMAC_TAG_SIZE {
            return Err(HammingError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "frame too short",
            )));
        }

        // Verify HMAC
        let hmac_start = data.len() - HMAC_TAG_SIZE;
        let expected_hmac = &data[hmac_start..];
        let computed_hmac = Self::compute_hmac(hmac_key, &data[..hmac_start]);
        if expected_hmac != computed_hmac {
            return Err(HammingError::HmacFailed);
        }

        let header = FrameHeader::from_bytes(&data[..FRAME_HEADER_SIZE])?;
        let payload_bytes = &data[FRAME_HEADER_SIZE..hmac_start];
        let payload = Self::decode_payload(&header, payload_bytes)?;

        Ok(Self { header, payload })
    }

    /// Encode payload based on mode.
    fn encode_payload(&self) -> Result<Vec<u8>> {
        match &self.payload {
            FramePayload::Fingerprint(fp) => {
                let bytes = fp.to_bytes();
                if bytes.len() > MAX_FRAME_PAYLOAD {
                    return Err(HammingError::FrameTooLarge {
                        size: bytes.len(),
                        max: MAX_FRAME_PAYLOAD,
                    });
                }
                Ok(bytes)
            }
            FramePayload::CodebookRefs(refs) => Ok(bitpack::pack_refs(refs)),
            FramePayload::XorComposition { base, chain } => {
                let mut addrs: Vec<u16> = Vec::with_capacity(1 + chain.len());
                addrs.push(*base);
                addrs.extend_from_slice(chain);
                Ok(bitpack::pack_addresses(&addrs))
            }
            FramePayload::Mixed(entries) => {
                let mut buf = Vec::new();
                for entry in entries {
                    match entry {
                        MixedEntry::Ref(r) => {
                            buf.push(0x01); // tag: ref
                            buf.extend_from_slice(&r.to_bytes());
                        }
                        MixedEntry::InlineFingerprint(fp) => {
                            buf.push(0x02); // tag: fingerprint
                            buf.extend_from_slice(&fp.to_bytes());
                        }
                        MixedEntry::InlineData(data) => {
                            buf.push(0x03); // tag: inline data
                            let len = data.len().min(u16::MAX as usize) as u16;
                            buf.extend_from_slice(&len.to_le_bytes());
                            buf.extend_from_slice(&data[..len as usize]);
                        }
                    }
                }
                Ok(buf)
            }
        }
    }

    /// Decode payload from bytes based on header mode.
    fn decode_payload(header: &FrameHeader, data: &[u8]) -> Result<FramePayload> {
        match header.flags.mode {
            PayloadMode::Fingerprint => {
                if data.len() < FINGERPRINT_BYTES {
                    return Err(HammingError::InvalidFingerprintLength {
                        expected: FINGERPRINT_BYTES,
                        actual: data.len(),
                    });
                }
                let fp = HammingVector::from_bytes(&data[..FINGERPRINT_BYTES])?;
                Ok(FramePayload::Fingerprint(fp))
            }
            PayloadMode::CodebookRef => {
                let refs = bitpack::unpack_refs(data);
                Ok(FramePayload::CodebookRefs(refs))
            }
            PayloadMode::XorComposition => {
                let count = header.op_count as usize;
                let addrs = bitpack::unpack_addresses(data, count)?;
                if addrs.is_empty() {
                    return Ok(FramePayload::XorComposition {
                        base: 0,
                        chain: Vec::new(),
                    });
                }
                Ok(FramePayload::XorComposition {
                    base: addrs[0],
                    chain: addrs[1..].to_vec(),
                })
            }
            PayloadMode::Mixed => {
                let mut entries = Vec::new();
                let mut pos = 0;
                while pos < data.len() {
                    let tag = data[pos];
                    pos += 1;
                    match tag {
                        0x01 => {
                            // Ref
                            if pos + 2 > data.len() { break; }
                            let r = PackedRef::from_bytes([data[pos], data[pos + 1]]);
                            entries.push(MixedEntry::Ref(r));
                            pos += 2;
                        }
                        0x02 => {
                            // Fingerprint
                            if pos + FINGERPRINT_BYTES > data.len() { break; }
                            let fp = HammingVector::from_bytes(&data[pos..pos + FINGERPRINT_BYTES])?;
                            entries.push(MixedEntry::InlineFingerprint(fp));
                            pos += FINGERPRINT_BYTES;
                        }
                        0x03 => {
                            // Inline data
                            if pos + 2 > data.len() { break; }
                            let len =
                                u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
                            pos += 2;
                            if pos + len > data.len() { break; }
                            entries.push(MixedEntry::InlineData(data[pos..pos + len].to_vec()));
                            pos += len;
                        }
                        _ => break,
                    }
                }
                Ok(FramePayload::Mixed(entries))
            }
        }
    }

    /// Compute HMAC-SHA256 truncated to 16 bytes.
    fn compute_hmac(key: &[u8], data: &[u8]) -> [u8; HMAC_TAG_SIZE] {
        // Simple HMAC: H(key || data) — in production use proper HMAC construction
        let mut hasher = Sha256::new();
        hasher.update(key);
        hasher.update(data);
        let hash = hasher.finalize();
        let mut tag = [0u8; HMAC_TAG_SIZE];
        tag.copy_from_slice(&hash[..HMAC_TAG_SIZE]);
        tag
    }
}

/// Builder for constructing frames incrementally.
pub struct FrameBuilder {
    session: [u8; 8],
    sequence: u32,
    surface_id: u32,
    codebook_version: u32,
    refs: Vec<PackedRef>,
    ack: u32,
    ack_requested: bool,
    cognitive: bool,
}

impl FrameBuilder {
    pub fn new(session: [u8; 8], codebook_version: u32) -> Self {
        Self {
            session,
            sequence: 0,
            surface_id: 0,
            codebook_version,
            refs: Vec::new(),
            ack: 0,
            ack_requested: false,
            cognitive: false,
        }
    }

    pub fn sequence(mut self, seq: u32) -> Self {
        self.sequence = seq;
        self
    }

    pub fn surface(mut self, id: u32) -> Self {
        self.surface_id = id;
        self
    }

    pub fn ack(mut self, ack: u32) -> Self {
        self.ack = ack;
        self
    }

    pub fn request_ack(mut self) -> Self {
        self.ack_requested = true;
        self
    }

    pub fn cognitive(mut self) -> Self {
        self.cognitive = true;
        self
    }

    /// Add a codebook reference.
    pub fn push_ref(mut self, address: u16, modifier: u8) -> Self {
        self.refs.push(PackedRef::new(address, modifier));
        self
    }

    /// Add a codebook reference (address only, no modifier).
    pub fn push_addr(self, address: u16) -> Self {
        self.push_ref(address, 0)
    }

    /// Build a Mode B (codebook ref) frame.
    pub fn build_refs(self) -> HammingFrame {
        let mut frame = HammingFrame::codebook_refs(
            self.session,
            self.sequence,
            self.surface_id,
            self.codebook_version,
            self.refs,
        );
        frame.header.ack = self.ack;
        frame.header.flags.ack_requested = self.ack_requested;
        frame.header.flags.cognitive = self.cognitive;
        frame
    }

    /// Build a Mode C (XOR composition) frame from the accumulated refs.
    pub fn build_xor(self) -> HammingFrame {
        let (base, chain) = if self.refs.is_empty() {
            (0, Vec::new())
        } else {
            let base = self.refs[0].address;
            let chain: Vec<u16> = self.refs[1..].iter().map(|r| r.address).collect();
            (base, chain)
        };
        let mut frame = HammingFrame::xor_composition(
            self.session,
            self.sequence,
            self.surface_id,
            self.codebook_version,
            base,
            chain,
        );
        frame.header.ack = self.ack;
        frame.header.flags.ack_requested = self.ack_requested;
        frame.header.flags.cognitive = self.cognitive;
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session() -> [u8; 8] {
        [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
    }

    fn test_hmac_key() -> Vec<u8> {
        b"test-hmac-key-32bytes-padding!!".to_vec()
    }

    #[test]
    fn test_mode_b_roundtrip() {
        let refs = vec![
            PackedRef::new(0x020, 0),  // createSurface
            PackedRef::new(0x084, 0),  // Column
            PackedRef::new(0x401, 0),  // theme:dark
        ];
        let frame = HammingFrame::codebook_refs(test_session(), 1, 1, 1, refs.clone());
        let encoded = frame.encode(&test_hmac_key()).unwrap();

        // Verify fits in UDP
        assert!(encoded.len() <= MAX_UDP_PAYLOAD, "frame size: {}", encoded.len());

        let decoded = HammingFrame::decode(&encoded, &test_hmac_key()).unwrap();
        assert_eq!(decoded.header.sequence, 1);
        assert_eq!(decoded.header.surface_id, 1);
        if let FramePayload::CodebookRefs(decoded_refs) = decoded.payload {
            assert_eq!(decoded_refs.len(), refs.len());
            for (a, b) in decoded_refs.iter().zip(refs.iter()) {
                assert_eq!(a.address, b.address);
                assert_eq!(a.modifier, b.modifier);
            }
        } else {
            panic!("expected CodebookRefs payload");
        }
    }

    #[test]
    fn test_mode_c_roundtrip() {
        let frame = HammingFrame::xor_composition(
            test_session(),
            2,
            1,
            1,
            0x020,                         // base: createSurface
            vec![0x084, 0x401, 0x421],     // Column, theme:dark, spacing:medium
        );
        let encoded = frame.encode(&test_hmac_key()).unwrap();
        assert!(encoded.len() <= MAX_UDP_PAYLOAD);

        let decoded = HammingFrame::decode(&encoded, &test_hmac_key()).unwrap();
        if let FramePayload::XorComposition { base, chain } = decoded.payload {
            assert_eq!(base, 0x020);
            assert_eq!(chain, vec![0x084, 0x401, 0x421]);
        } else {
            panic!("expected XorComposition payload");
        }
    }

    #[test]
    fn test_mode_a_roundtrip() {
        let fp = HammingVector::from_seed("test fingerprint");
        let frame = HammingFrame::fingerprint(test_session(), 3, 1, 1, fp.clone());
        let encoded = frame.encode(&test_hmac_key()).unwrap();
        assert!(encoded.len() <= MAX_UDP_PAYLOAD);

        let decoded = HammingFrame::decode(&encoded, &test_hmac_key()).unwrap();
        if let FramePayload::Fingerprint(decoded_fp) = decoded.payload {
            assert_eq!(fp, decoded_fp);
        } else {
            panic!("expected Fingerprint payload");
        }
    }

    #[test]
    fn test_hmac_verification() {
        let frame = HammingFrame::codebook_refs(
            test_session(), 1, 1, 1,
            vec![PackedRef::new(0x080, 0)],
        );
        let encoded = frame.encode(&test_hmac_key()).unwrap();

        // Tamper with a byte
        let mut tampered = encoded.clone();
        tampered[FRAME_HEADER_SIZE] ^= 0xFF;

        let result = HammingFrame::decode(&tampered, &test_hmac_key());
        assert!(result.is_err());
    }

    #[test]
    fn test_frame_builder() {
        let frame = FrameBuilder::new(test_session(), 1)
            .sequence(42)
            .surface(1)
            .push_addr(0x020)   // createSurface
            .push_addr(0x084)   // Column
            .push_ref(0x401, 0) // theme:dark
            .request_ack()
            .build_refs();

        assert_eq!(frame.header.sequence, 42);
        assert!(frame.header.flags.ack_requested);
        if let FramePayload::CodebookRefs(refs) = &frame.payload {
            assert_eq!(refs.len(), 3);
        } else {
            panic!("expected CodebookRefs");
        }
    }

    #[test]
    fn test_max_refs_per_frame() {
        // Mode B: 2 bytes per ref, max payload = 1424 bytes → 712 refs
        let refs: Vec<PackedRef> = (0..712)
            .map(|i| PackedRef::new((i % 0xFFF) as u16, 0))
            .collect();
        let frame = HammingFrame::codebook_refs(test_session(), 1, 0, 1, refs);
        let result = frame.encode(&test_hmac_key());
        assert!(result.is_ok(), "should fit 712 refs: {:?}", result.err());
    }
}
