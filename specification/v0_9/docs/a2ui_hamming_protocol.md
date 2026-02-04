# A2UI Hamming Protocol Specification

**Version:** 0.1-draft
**Status:** Prototype
**Depends on:** A2UI v0.9, ladybug-rs BindSpace, firefly HammingVector

## 1. Overview

The A2UI Hamming Protocol replaces JSON-based message exchange with a binary protocol
built on **10,000-bit Hamming fingerprints** and a **pre-shared codebook**. Every A2UI
operation—component creation, data binding, style application, user action—is encoded
as a codebook reference or XOR composition of references, transmitted in a single
UDP-sized frame (≤1,472 bytes).

### 1.1 Design Principles

1. **No JSON in the hot path.** JSON exists only for initial capability negotiation
   and as a debug/fallback mode. All rendering operations use bitpacked Hamming.
2. **Codebook-first semantics.** A 4,096-entry codebook (12-bit addresses) maps
   fingerprints to A2UI operations. Complex operations are XOR compositions.
3. **UDP-native framing.** A single 10kbit fingerprint (1,250 bytes) plus a 32-byte
   frame header fits within standard MTU (1,500 bytes).
4. **Cognitive self-management.** The codebook itself is a ladybug-rs BindSpace
   that can reason about its own optimization via NARS inference.
5. **Zero-copy transport.** Arrow Flight streams RecordBatches of fingerprints
   for bulk operations. Single operations use UDP frames.

### 1.2 Protocol Stack

```
┌─────────────────────────────────────────────────────┐
│  Application Layer: A2UI Surfaces & Components      │
├─────────────────────────────────────────────────────┤
│  Semantic Layer: Codebook (4096 × 10kbit entries)   │
│  ┌─────────────────────────────────────────────┐    │
│  │ B-tree indexed │ XOR composable │ Versioned │    │
│  └─────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────┤
│  Encoding Layer: Bitpacked Hamming + ECC            │
├─────────────────────────────────────────────────────┤
│  Transport Layer                                     │
│  ┌──────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │ UDP frame │  │ gRPC stream  │  │ Arrow Flight │  │
│  │ (single)  │  │ (ordered)    │  │ (bulk batch) │  │
│  └──────────┘  └──────────────┘  └──────────────┘  │
├─────────────────────────────────────────────────────┤
│  Auth Layer: HMAC-SHA256 per frame + session token  │
└─────────────────────────────────────────────────────┘
```

## 2. Codebook

### 2.1 Structure

The codebook contains **4,096 entries** addressed by 12-bit indices (`0x000`–`0xFFF`).
Each entry is a 10,000-bit fingerprint generated deterministically from the semantic
content it represents (using SHA-256 seeded LFSR expansion, compatible with
ladybug-rs `fingerprint()` and firefly `HammingVector`).

```
Codebook Entry:
┌──────────┬──────────────┬───────────────────────────┐
│ Address  │ Fingerprint  │ Semantic Binding           │
│ 12 bits  │ 10,000 bits  │ (implicit in fingerprint)  │
│          │ (1,250 bytes)│                             │
└──────────┴──────────────┴───────────────────────────┘
```

### 2.2 Address Zones

| Zone             | Range           | Count | Purpose                              |
|------------------|-----------------|-------|--------------------------------------|
| **Protocol**     | `0x000`–`0x01F` | 32    | Frame types, handshake, versioning   |
| **Messages**     | `0x020`–`0x07F` | 96    | A2UI message types & envelopes       |
| **Components**   | `0x080`–`0x1FF` | 384   | Standard catalog components          |
| **Properties**   | `0x200`–`0x3FF` | 512   | Component properties & values        |
| **Styles**       | `0x400`–`0x5FF` | 512   | Theme, layout, colors, typography    |
| **Data Ops**     | `0x600`–`0x7FF` | 512   | Data model paths, bindings, updates  |
| **Actions**      | `0x800`–`0x9FF` | 512   | User actions, events, callbacks      |
| **Composites**   | `0xA00`–`0xBFF` | 512   | Pre-composed multi-operation entries  |
| **Dynamic**      | `0xC00`–`0xEFF` | 768   | Session-learned patterns, user prefs |
| **Cognitive**    | `0xF00`–`0xFFF` | 256   | NARS/ladybug-rs cognitive operations |

### 2.3 XOR Composition

Complex A2UI operations are expressed as XOR compositions of codebook entries:

```
createSurface("main", theme=dark, root=Column) =
    CODEBOOK[0x020]  // createSurface
  ⊕ CODEBOOK[0x080]  // Column component
  ⊕ CODEBOOK[0x401]  // theme:dark
```

The XOR operation is self-inverse: applying the same composition twice yields the
original fingerprint. This enables:

- **Delta encoding**: `update = old_state ⊕ new_state`
- **Rollback**: `old_state = new_state ⊕ update`
- **Composition verification**: Hamming distance from expected result detects errors

### 2.4 B-tree Index

The codebook is indexed by a B-tree over the first 64 bits of each fingerprint.
This provides O(log n) lookup by prefix and enables range queries over semantically
similar operations (fingerprints generated from related content will share prefixes
due to the SHA-256 seeding).

```
B-tree Node (order=64):
┌──────────────────────────────────────────────────┐
│ keys[63]: first 64 bits of fingerprint           │
│ children[64]: pointers to child nodes or entries │
│ count: number of keys in this node               │
└──────────────────────────────────────────────────┘
```

### 2.5 Versioning

Codebook versions are tracked as a monotonic 32-bit counter. Version updates are
distributed as XOR deltas:

```
CodebookDelta:
┌─────────┬───────────┬─────────┬─────────────────────┐
│ version │ base_ver  │ n_deltas│ deltas[]             │
│ u32     │ u32       │ u16     │ (addr: u12, xor_δ)  │
└─────────┴───────────┴─────────┴─────────────────────┘

Each delta: address (12 bits) + XOR mask (10,000 bits) = 1,252 bytes
Apply: codebook[addr] = codebook[addr] ⊕ xor_δ
Rollback: codebook[addr] = codebook[addr] ⊕ xor_δ  (same operation)
```

## 3. Frame Format

### 3.1 UDP Frame

Total frame size: **≤1,472 bytes** (fits in standard 1,500 MTU minus IP+UDP headers).

```
Hamming Frame (max 1,472 bytes):
┌──────────────────────────────────────────────────────────┐
│ Header (32 bytes)                                         │
│ ┌────────┬────────┬────────┬────────┬────────┬────────┐  │
│ │ magic  │ ver    │ flags  │ seq    │ ack    │ session│  │
│ │ 2B     │ 1B     │ 1B     │ 4B     │ 4B     │ 8B     │  │
│ ├────────┴────────┴────────┴────────┴────────┴────────┤  │
│ │ surface_id      │ op_count│ codebook_ver │ reserved  │  │
│ │ 4B              │ 2B      │ 4B           │ 2B        │  │
│ └─────────────────┴─────────┴──────────────┴──────────┘  │
├──────────────────────────────────────────────────────────┤
│ Payload (max 1,424 bytes)                                 │
│ ┌──────────────────────────────────────────────────────┐  │
│ │ Mode A: Single fingerprint (1,250 bytes)             │  │
│ │   fingerprint[1250] + padding[174]                   │  │
│ │                                                      │  │
│ │ Mode B: Codebook references (up to 948 refs)         │  │
│ │   ref[]: address(12 bits) + modifier(optional)       │  │
│ │                                                      │  │
│ │ Mode C: XOR composition chain                        │  │
│ │   base_addr(12b) + xor_addr[](12b each)             │  │
│ │                                                      │  │
│ │ Mode D: Mixed (refs + inline fingerprint fragments)  │  │
│ └──────────────────────────────────────────────────────┘  │
├──────────────────────────────────────────────────────────┤
│ Auth Tag (16 bytes): HMAC-SHA256 truncated               │
└──────────────────────────────────────────────────────────┘
```

### 3.2 Header Fields

| Field          | Size  | Description                                    |
|----------------|-------|------------------------------------------------|
| `magic`        | 2B    | `0xA2B1` — identifies A2UI Hamming frame       |
| `ver`          | 1B    | Protocol version (currently `0x01`)            |
| `flags`        | 1B    | Bitfield: mode(2b), compressed(1b), ack_req(1b), more_fragments(1b), cognitive(1b), reserved(2b) |
| `seq`          | 4B    | Sequence number (monotonic per session)        |
| `ack`          | 4B    | Last acknowledged sequence number              |
| `session`      | 8B    | Session identifier (first 8 bytes of SHA-256)  |
| `surface_id`   | 4B    | Target surface (0 = broadcast / control)       |
| `op_count`     | 2B    | Number of operations in payload                |
| `codebook_ver` | 4B    | Sender's codebook version                      |
| `reserved`     | 2B    | Reserved for future use                        |

### 3.3 Flag Bits

```
Bit 7-6: Payload mode
  00 = Mode A (single fingerprint)
  01 = Mode B (codebook references)
  10 = Mode C (XOR composition chain)
  11 = Mode D (mixed)
Bit 5: Compressed (LZ4 applied to payload)
Bit 4: ACK requested
Bit 3: More fragments follow
Bit 2: Cognitive frame (routed to NARS/ladybug-rs)
Bit 1-0: Reserved
```

### 3.4 Payload Modes

**Mode A — Single Fingerprint:**
Used for novel operations not in the codebook, or for codebook updates.
Carries one full 10,000-bit (1,250-byte) fingerprint.

**Mode B — Codebook References:**
Most common mode. Each reference is 12 bits (codebook address) plus optional
4-bit modifier for common property overrides. At 2 bytes per reference,
a single frame can carry up to **712 operations**.

```
Reference Entry (2 bytes):
┌──────────────┬──────────┐
│ address      │ modifier │
│ 12 bits      │ 4 bits   │
└──────────────┴──────────┘
```

**Mode C — XOR Composition Chain:**
A base address followed by a sequence of addresses to XOR together.
Result = codebook[base] ⊕ codebook[addr1] ⊕ codebook[addr2] ⊕ ...

```
Composition (variable):
┌──────────────┬──────────────┬──────────────┬─────┐
│ base_addr    │ xor_addr[0]  │ xor_addr[1]  │ ... │
│ 12 bits      │ 12 bits      │ 12 bits      │     │
└──────────────┴──────────────┴──────────────┴─────┘
```

**Mode D — Mixed:**
Combines codebook references with inline fingerprint fragments for
partially-known operations.

## 4. Transport Bindings

### 4.1 UDP (Low-latency, Best-effort)

Direct UDP frames as described in Section 3. Used for real-time UI updates
where occasional packet loss is acceptable (the next frame will carry the
full state or a correction).

- **Port:** Negotiated during gRPC handshake
- **MTU:** 1,500 bytes (standard Ethernet)
- **Reliability:** Application-level ACK via `ack` field
- **Ordering:** Sequence numbers; receiver may reorder within window

### 4.2 gRPC (Reliable, Ordered)

Bidirectional streaming over HTTP/2. Used for:
- Session establishment and codebook synchronization
- Reliable delivery of critical operations (surface creation/deletion)
- Fallback when UDP is unavailable (firewalled environments)

See `proto/a2ui/hamming.proto` for service definitions.

### 4.3 Arrow Flight (Bulk Batch)

Used for bulk operations via ladybug-rs:
- Initial codebook download (4,096 × 1,250 bytes ≈ 5MB as RecordBatch)
- Batch component updates (many surfaces at once)
- Codebook version sync across cluster

Arrow schema for codebook RecordBatch:
```
Schema:
  address:     uint16      (codebook address)
  fingerprint: binary(1250) (10,000-bit fingerprint, bitpacked)
  zone:        utf8         (address zone name)
  version:     uint32       (entry version)
  semantic:    utf8         (optional debug label)
```

## 5. Session Lifecycle

### 5.1 Handshake (over gRPC)

```
1. Client → Server: InitSession {
     client_capabilities,     // A2UI client capabilities
     supported_codebook_ver,  // latest codebook version client has
     udp_port,                // port for UDP frames (0 = gRPC only)
     auth_token               // session authentication
   }

2. Server → Client: SessionEstablished {
     session_id,              // 8-byte session identifier
     codebook_ver,            // server's codebook version
     codebook_delta,          // XOR delta if client is behind
     hmac_key,                // per-session HMAC key (encrypted)
     udp_endpoint             // server's UDP endpoint (if applicable)
   }

3. If codebook versions diverge significantly:
   Server → Client: Full codebook via Arrow Flight DoGet
```

### 5.2 Rendering Flow

```
Agent (LLM) generates A2UI intent
        │
        ▼
  ┌─────────────────┐
  │ Hamming Encoder  │  Resolves intent to codebook references
  │ (server-side)    │  or XOR compositions
  └────────┬────────┘
           │
           ▼
  ┌─────────────────┐
  │ Frame Builder    │  Packs references into UDP/gRPC frames
  └────────┬────────┘
           │
     ┌─────┴──────┐
     │             │
  UDP frame    gRPC stream
     │             │
     └─────┬───────┘
           │
           ▼
  ┌─────────────────┐
  │ Frame Decoder    │  Unpacks frame, resolves codebook refs
  │ (client-side)    │
  └────────┬────────┘
           │
           ▼
  ┌─────────────────┐
  │ Codebook Resolve │  Looks up fingerprints in local B-tree
  └────────┬────────┘
           │
           ▼
  ┌─────────────────┐
  │ A2UI Renderer    │  Applies operations to surface
  │ (Lit/Angular)    │
  └─────────────────┘
```

### 5.3 User Actions (Client → Server)

User interactions are encoded as codebook references and sent back:

```
Button click → CODEBOOK[0x800] ⊕ CODEBOOK[button_action_addr]
Text input   → CODEBOOK[0x801] + inline UTF-8 payload
Scroll       → CODEBOOK[0x802] + 4-byte position
```

## 6. Cognitive Codebook (ladybug-rs Integration)

### 6.1 Self-Sufficient Operations

When the codebook runs on ladybug-rs, it gains cognitive capabilities:

1. **Pattern Learning**: NARS inference identifies frequently co-occurring
   operations and creates composite entries in the Dynamic zone (0xC00–0xEFF).

2. **Semantic Search**: Given a novel operation, find the nearest codebook
   entry by Hamming distance (using HDR cascade index).

3. **Codebook Optimization**: Cognitive layer analyzes usage patterns and
   proposes codebook reorganization to minimize average frame size.

4. **Predictive Prefetch**: Based on causal reasoning, predict next likely
   operations and prefetch to client.

### 6.2 BindSpace Mapping

```
ladybug-rs BindSpace Allocation for A2UI Codebook:
┌────────────────────────────────────────────────────┐
│ Surface Zone (0x00-0x0F):                          │
│   0x00: A2UI Control Channel                       │
│   0x01: Codebook Sync Channel                      │
│   0x02: Cognitive Feedback Channel                 │
├────────────────────────────────────────────────────┤
│ Fluid Zone (0x10-0x7F):                            │
│   0x10-0x1F: Active Session Contexts               │
│   0x20-0x3F: Working Memory (current UI state)     │
│   0x40-0x5F: Prediction Buffer                     │
│   0x60-0x7F: Cognitive Inference Workspace          │
├────────────────────────────────────────────────────┤
│ Nodes Zone (0x80-0xFF):                            │
│   Codebook entries stored as BindNodes             │
│   fingerprint = codebook entry fingerprint         │
│   label = semantic description                     │
│   qualia_index = address zone                      │
│   edges = XOR composition relationships            │
└────────────────────────────────────────────────────┘
```

### 6.3 NARS Reasoning on Codebook

ladybug-rs NARS can perform inference over the codebook:

```
// Observation: Button clicks often follow Card renders
<card_render --> precedes_button_click>. %0.85;0.90%

// Inference: Pre-compose card+button into single entry
<card_with_button --> composite_optimization>. %0.85;0.81%

// Result: New Dynamic zone entry at 0xC00+
codebook[0xC00] = codebook[0x08A] ⊕ codebook[0x081]  // Card ⊕ Button
```

## 7. Firefly Integration (Graph Execution Language)

### 7.1 GEL Rendering Pipeline

Each A2UI surface maps to a firefly execution graph:

```
FireflyNode: Layout
  ├── executor: Native
  ├── resonance: fingerprint("a2ui.layout.resolve")
  ├── inputs: [component_tree, viewport_size]
  └── outputs: [positioned_components]
        │
        ▼
FireflyNode: Style
  ├── executor: Native
  ├── resonance: fingerprint("a2ui.style.apply")
  ├── inputs: [positioned_components, theme]
  └── outputs: [styled_components]
        │
        ▼
FireflyNode: Render
  ├── executor: WASM (client-side)
  ├── resonance: fingerprint("a2ui.render.commit")
  ├── inputs: [styled_components]
  └── outputs: [dom_mutations]
```

### 7.2 Distributed Execution

The graph can split across client/server boundary:

```
Server (firefly):           Network:          Client (WASM):
┌──────────────┐                              ┌──────────────┐
│ AI Generation│                              │ Layout Engine │
│ → Hamming    │──── UDP/gRPC frames ────────▶│ → Style      │
│   Encoding   │                              │ → DOM Render │
│ → Codebook   │◀─── User Action frames ─────│ → Event      │
│   Resolve    │                              │   Capture    │
└──────────────┘                              └──────────────┘
```

FireflyPackets carry the same 10kbit HammingVectors, so there is zero
translation cost between firefly execution and A2UI Hamming transport.

## 8. Error Correction

### 8.1 Hamming Distance Validation

Each codebook entry has known Hamming distances to related entries.
The receiver validates decoded fingerprints against expected distances:

```
received = decode(frame)
nearest = codebook.nearest(received)
distance = hamming(received, nearest.fingerprint)

if distance == 0:
    exact_match → execute operation
elif distance < ERROR_THRESHOLD (e.g., 50 bits):
    likely_corruption → correct to nearest entry
else:
    unknown_operation → request retransmission or fall back to gRPC
```

### 8.2 Frame-Level CRC

Each frame includes a CRC32 checksum in the auth tag for transport-level
integrity verification before Hamming-level processing.

## 9. Compression

### 9.1 LZ4 Frame Compression

When the `compressed` flag is set, the payload is LZ4-compressed before
framing. This is effective for Mode B (codebook references) where
sequential addresses often share prefixes.

### 9.2 Codebook-Level Compression

The codebook itself achieves compression through semantic encoding:
- A full A2UI JSON message (1-10KB) → single 12-bit codebook reference
- Common multi-component layouts → single composite reference
- This achieves **100x-1000x compression** over JSON for repeated patterns

## 10. Security

### 10.1 Session Authentication

- Initial auth via gRPC with standard bearer tokens
- Session key derived during handshake (HKDF-SHA256)
- Each UDP frame authenticated with HMAC-SHA256 (truncated to 16 bytes)

### 10.2 Codebook Integrity

- Codebook versions are cryptographically chained (hash of version N
  includes hash of version N-1)
- XOR deltas include integrity proofs
- Tampered codebook entries detected by Hamming distance anomalies

## 11. Compatibility

### 11.1 JSON Fallback

Clients that don't support the Hamming protocol can negotiate JSON mode
during the gRPC handshake. The server maintains a JSON encoder alongside
the Hamming encoder.

### 11.2 A2UI Version Compatibility

The Hamming protocol is orthogonal to A2UI spec versions. Codebook entries
are tagged with the A2UI version they encode, allowing mixed-version
operation.
