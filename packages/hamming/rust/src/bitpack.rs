//! Bitpacking utilities for codebook references and frame payloads.
//!
//! Packs 12-bit codebook addresses and 4-bit modifiers into compact
//! byte sequences for UDP frame payloads.

use crate::error::{HammingError, Result};

/// A single codebook reference: 12-bit address + 4-bit modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedRef {
    /// 12-bit codebook address (0x000–0xFFF)
    pub address: u16,
    /// 4-bit modifier (0x0–0xF) for property overrides
    pub modifier: u8,
}

impl PackedRef {
    pub fn new(address: u16, modifier: u8) -> Self {
        Self {
            address: address & 0xFFF,
            modifier: modifier & 0xF,
        }
    }

    /// Pack into 2 bytes: [addr_hi(4) | modifier(4)] [addr_lo(8)]
    pub fn to_bytes(&self) -> [u8; 2] {
        let hi = ((self.address >> 8) as u8 & 0x0F) | (self.modifier << 4);
        let lo = (self.address & 0xFF) as u8;
        [hi, lo]
    }

    /// Unpack from 2 bytes.
    pub fn from_bytes(bytes: [u8; 2]) -> Self {
        let modifier = (bytes[0] >> 4) & 0x0F;
        let addr_hi = (bytes[0] & 0x0F) as u16;
        let addr_lo = bytes[1] as u16;
        let address = (addr_hi << 8) | addr_lo;
        Self { address, modifier }
    }
}

/// Pack a sequence of 12-bit addresses (no modifiers) densely.
///
/// Three 12-bit values pack into 36 bits = 4.5 bytes.
/// We use 3 addresses → 4 bytes + leftover byte for odd counts.
pub fn pack_addresses(addresses: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity((addresses.len() * 3 + 1) / 2);
    let mut i = 0;

    while i + 1 < addresses.len() {
        let a = addresses[i] & 0xFFF;
        let b = addresses[i + 1] & 0xFFF;

        // Pack two 12-bit values into 3 bytes
        // Byte 0: a[11:4]
        // Byte 1: a[3:0] | b[11:8]
        // Byte 2: b[7:0]
        out.push((a >> 4) as u8);
        out.push(((a & 0xF) << 4 | (b >> 8) & 0xF) as u8);
        out.push((b & 0xFF) as u8);

        i += 2;
    }

    // Handle odd trailing address
    if i < addresses.len() {
        let a = addresses[i] & 0xFFF;
        out.push((a >> 4) as u8);
        out.push(((a & 0xF) << 4) as u8);
    }

    out
}

/// Unpack densely packed 12-bit addresses.
pub fn unpack_addresses(data: &[u8], count: usize) -> Result<Vec<u16>> {
    let mut addresses = Vec::with_capacity(count);
    let mut i = 0;
    let mut remaining = count;

    while remaining >= 2 && i + 2 < data.len() {
        let a = ((data[i] as u16) << 4) | ((data[i + 1] as u16) >> 4);
        let b = (((data[i + 1] & 0xF) as u16) << 8) | (data[i + 2] as u16);
        addresses.push(a & 0xFFF);
        addresses.push(b & 0xFFF);
        i += 3;
        remaining -= 2;
    }

    if remaining == 1 && i + 1 < data.len() {
        let a = ((data[i] as u16) << 4) | ((data[i + 1] as u16) >> 4);
        addresses.push(a & 0xFFF);
    }

    Ok(addresses)
}

/// Pack a sequence of PackedRef (address + modifier) into bytes.
pub fn pack_refs(refs: &[PackedRef]) -> Vec<u8> {
    let mut out = Vec::with_capacity(refs.len() * 2);
    for r in refs {
        let bytes = r.to_bytes();
        out.extend_from_slice(&bytes);
    }
    out
}

/// Unpack PackedRef sequence from bytes.
pub fn unpack_refs(data: &[u8]) -> Vec<PackedRef> {
    data.chunks_exact(2)
        .map(|chunk| PackedRef::from_bytes([chunk[0], chunk[1]]))
        .collect()
}

/// Inline data encoder: packs small values (strings, numbers) with
/// a length prefix into the frame payload alongside codebook refs.
pub struct InlineEncoder {
    buffer: Vec<u8>,
}

impl InlineEncoder {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Append a UTF-8 string (max 255 bytes).
    pub fn push_string(&mut self, s: &str) -> Result<()> {
        let bytes = s.as_bytes();
        if bytes.len() > 255 {
            return Err(HammingError::FrameTooLarge {
                size: bytes.len(),
                max: 255,
            });
        }
        self.buffer.push(0x01); // type tag: string
        self.buffer.push(bytes.len() as u8);
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    /// Append a u32 value.
    pub fn push_u32(&mut self, val: u32) {
        self.buffer.push(0x02); // type tag: u32
        self.buffer.extend_from_slice(&val.to_le_bytes());
    }

    /// Append a f64 value.
    pub fn push_f64(&mut self, val: f64) {
        self.buffer.push(0x03); // type tag: f64
        self.buffer.extend_from_slice(&val.to_le_bytes());
    }

    /// Append raw bytes (max 255 bytes).
    pub fn push_bytes(&mut self, data: &[u8]) -> Result<()> {
        if data.len() > 255 {
            return Err(HammingError::FrameTooLarge {
                size: data.len(),
                max: 255,
            });
        }
        self.buffer.push(0x04); // type tag: raw bytes
        self.buffer.push(data.len() as u8);
        self.buffer.extend_from_slice(data);
        Ok(())
    }

    pub fn finish(self) -> Vec<u8> {
        self.buffer
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Inline data decoder: reads typed values from a byte stream.
pub struct InlineDecoder<'a> {
    data: &'a [u8],
    pos: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InlineValue {
    String(String),
    U32(u32),
    F64(f64),
    Bytes(Vec<u8>),
}

impl<'a> InlineDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn next(&mut self) -> Option<InlineValue> {
        if self.pos >= self.data.len() {
            return None;
        }

        let tag = self.data[self.pos];
        self.pos += 1;

        match tag {
            0x01 => {
                // String
                if self.pos >= self.data.len() { return None; }
                let len = self.data[self.pos] as usize;
                self.pos += 1;
                if self.pos + len > self.data.len() { return None; }
                let s = String::from_utf8_lossy(&self.data[self.pos..self.pos + len]).to_string();
                self.pos += len;
                Some(InlineValue::String(s))
            }
            0x02 => {
                // U32
                if self.pos + 4 > self.data.len() { return None; }
                let val = u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().ok()?);
                self.pos += 4;
                Some(InlineValue::U32(val))
            }
            0x03 => {
                // F64
                if self.pos + 8 > self.data.len() { return None; }
                let val = f64::from_le_bytes(self.data[self.pos..self.pos + 8].try_into().ok()?);
                self.pos += 8;
                Some(InlineValue::F64(val))
            }
            0x04 => {
                // Raw bytes
                if self.pos >= self.data.len() { return None; }
                let len = self.data[self.pos] as usize;
                self.pos += 1;
                if self.pos + len > self.data.len() { return None; }
                let bytes = self.data[self.pos..self.pos + len].to_vec();
                self.pos += len;
                Some(InlineValue::Bytes(bytes))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packed_ref_roundtrip() {
        let r = PackedRef::new(0xABC, 0x5);
        let bytes = r.to_bytes();
        let r2 = PackedRef::from_bytes(bytes);
        assert_eq!(r.address, r2.address);
        assert_eq!(r.modifier, r2.modifier);
    }

    #[test]
    fn test_pack_addresses_even() {
        let addrs = vec![0x080, 0x081, 0x400, 0x401];
        let packed = pack_addresses(&addrs);
        let unpacked = unpack_addresses(&packed, 4).unwrap();
        assert_eq!(addrs, unpacked);
    }

    #[test]
    fn test_pack_addresses_odd() {
        let addrs = vec![0x020, 0x080, 0x401];
        let packed = pack_addresses(&addrs);
        let unpacked = unpack_addresses(&packed, 3).unwrap();
        assert_eq!(addrs, unpacked);
    }

    #[test]
    fn test_pack_refs() {
        let refs = vec![
            PackedRef::new(0x080, 0),
            PackedRef::new(0x081, 3),
            PackedRef::new(0xFFF, 0xF),
        ];
        let packed = pack_refs(&refs);
        let unpacked = unpack_refs(&packed);
        assert_eq!(refs, unpacked);
    }

    #[test]
    fn test_inline_encoder_decoder() {
        let mut enc = InlineEncoder::new();
        enc.push_string("hello").unwrap();
        enc.push_u32(42);
        enc.push_f64(3.14);

        let data = enc.finish();
        let mut dec = InlineDecoder::new(&data);

        assert_eq!(dec.next(), Some(InlineValue::String("hello".into())));
        assert_eq!(dec.next(), Some(InlineValue::U32(42)));
        if let Some(InlineValue::F64(v)) = dec.next() {
            assert!((v - 3.14).abs() < 1e-10);
        } else {
            panic!("expected f64");
        }
        assert_eq!(dec.next(), None);
    }
}
