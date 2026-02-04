# A2UI Hamming Protocol — Architecture & Integration

## Overview

The A2UI Hamming Protocol replaces JSON-based UI message exchange with a binary
protocol built on **10,000-bit Hamming fingerprints**. This document describes
the architecture, integration points with ladybug-rs and firefly, and the
prototype codebase structure.

## System Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        Server (Rust)                                     │
│                                                                          │
│  ┌──────────┐    ┌──────────────────┐    ┌─────────────────────────┐    │
│  │ AI Agent │───▶│ Hamming Encoder  │───▶│ Frame Builder           │    │
│  │ (LLM)   │    │ (codebook refs)  │    │ (UDP/gRPC frames)       │    │
│  └──────────┘    └──────────────────┘    └──────────┬──────────────┘    │
│                                                      │                   │
│  ┌──────────────────────────────────────────────────┐│                   │
│  │ ladybug-rs (Cognitive Codebook)                  ││                   │
│  │ ┌──────────┐ ┌────────┐ ┌──────────┐            ││                   │
│  │ │BindSpace │ │ NARS   │ │ HDR      │            ││                   │
│  │ │(codebook │ │inference│ │cascade   │            ││                   │
│  │ │ storage) │ │(learn) │ │(search)  │            ││                   │
│  │ └──────────┘ └────────┘ └──────────┘            ││                   │
│  └──────────────────────────────────────────────────┘│                   │
│                                                      │                   │
│  ┌──────────────────────────────────────────────────┐│                   │
│  │ firefly (GEL Execution Graph)                    ││                   │
│  │ ┌──────────┐ ┌────────────┐ ┌───────────────┐   ││                   │
│  │ │Firefly   │ │ Graph      │ │ Packet        │   ││                   │
│  │ │Nodes     │ │ Executor   │ │ Router        │   ││                   │
│  │ │(pipeline)│ │ (topo sort)│ │ (distribute)  │   ││                   │
│  │ └──────────┘ └────────────┘ └───────────────┘   ││                   │
│  └──────────────────────────────────────────────────┘│                   │
│                                                      │                   │
├──────────────────────────────────────────────────────┼───────────────────┤
│                      Transport                        │                   │
│  ┌──────────┐  ┌──────────────┐  ┌──────────────┐   │                   │
│  │UDP frames│  │gRPC bistream │  │Arrow Flight  │   │                   │
│  │(realtime)│  │(reliable)    │  │(bulk sync)   │   │                   │
│  └────┬─────┘  └──────┬───────┘  └──────┬───────┘   │                   │
├───────┼────────────────┼─────────────────┼───────────┼───────────────────┤
│       │                │                 │           │                   │
│       ▼                ▼                 ▼                               │
│  ┌──────────────────────────────────────────────────────────────┐       │
│  │                   Client (WASM + TypeScript)                  │       │
│  │                                                               │       │
│  │  ┌──────────────┐    ┌──────────────┐    ┌───────────────┐   │       │
│  │  │ Frame Decoder │───▶│ Codebook     │───▶│ A2UI Renderer │   │       │
│  │  │ (WASM)       │    │ Resolver     │    │ (Lit/Angular) │   │       │
│  │  └──────────────┘    │ (WASM)       │    └───────────────┘   │       │
│  │                      └──────────────┘                         │       │
│  └──────────────────────────────────────────────────────────────┘       │
└─────────────────────────────────────────────────────────────────────────┘
```

## Codebase Structure

```
packages/hamming/
├── rust/                          # Core Rust library
│   ├── Cargo.toml                 # Dependencies and feature flags
│   ├── build.rs                   # Protobuf codegen
│   └── src/
│       ├── lib.rs                 # Library root, constants
│       ├── error.rs               # Error types
│       ├── vector.rs              # 10kbit HammingVector (SIMD-accelerated)
│       ├── bitpack.rs             # Bitpacking: 12-bit addresses, PackedRef
│       ├── zones.rs               # Codebook address zones & well-known addresses
│       ├── codebook.rs            # B-tree indexed codebook with XOR composition
│       ├── frame.rs               # UDP frame codec (header + payload + HMAC)
│       ├── xor_stream.rs          # Codebook versioning, delta sync, chunking
│       ├── wasm.rs                # WASM bindings (wasm-bindgen)
│       └── transport/
│           ├── mod.rs             # Shared transport state, session management
│           ├── server.rs          # gRPC server: encode A2UI ops → Hamming frames
│           └── client.rs          # gRPC client: decode frames → A2UI operations
├── typescript/                    # Thin TypeScript wrapper
│   ├── package.json
│   ├── tsconfig.json
│   └── src/
│       └── index.ts               # HammingTransport class, codebook addresses
└── ARCHITECTURE.md                # This file

proto/a2ui/
└── hamming.proto                  # gRPC service definitions

specification/v0_9/docs/
└── a2ui_hamming_protocol.md       # Protocol specification
```

## Integration Points

### ladybug-rs Integration

ladybug-rs provides the cognitive backbone for the codebook:

| ladybug-rs Component | A2UI Hamming Usage |
|----------------------|-------------------|
| **BindSpace** | Stores codebook entries as BindNodes; address zones map to BindSpace surface/fluid/nodes |
| **fingerprint()** | Same SHA-256 LFSR seeding generates identical 10kbit vectors |
| **Hamming distance (SIMD)** | Frame error correction and semantic search |
| **HDR cascade** | Fast nearest-neighbor lookup in codebook |
| **NARS inference** | Learns usage patterns, proposes composite entries |
| **Arrow Flight** | Bulk codebook sync (DoGet streams RecordBatches of entries) |
| **DoAction** | Encode/bind/distance operations exposed as Flight actions |

**BindSpace Mapping:**

```
Surface (0x00-0x0F):
  0x00 → A2UI control channel
  0x01 → Codebook sync channel
  0x02 → Cognitive feedback channel

Fluid (0x10-0x7F):
  0x10-0x1F → Active session contexts
  0x20-0x3F → Working memory (current UI state)
  0x40-0x5F → Prediction buffer
  0x60-0x7F → NARS inference workspace

Nodes (0x80-0xFF):
  Codebook entries stored as BindNodes
```

### firefly Integration

firefly provides the execution layer for distributed rendering:

| firefly Component | A2UI Hamming Usage |
|-------------------|-------------------|
| **HammingVector** | Same 10kbit fingerprints (157 × u64), binary compatible |
| **FireflyNode** | Each A2UI rendering stage is a node in the execution graph |
| **FireflyEdge** | XOR-bound edges encode UI composition relationships |
| **FireflyPacket** | Transport packets carry the same 10kbit payloads |
| **Graph Executor** | Topological execution of layout → style → render pipeline |
| **LadybugDB** | Declarative code graph for UI templates and patterns |

**GEL Pipeline for A2UI:**

```
[Encode Node] → [Layout Node] → [Style Node] → [Render Node]
  executor: Native      executor: WASM       executor: WASM
  resonance: encode_fp  resonance: layout_fp resonance: render_fp
  runs on: server       runs on: client      runs on: client
```

The network boundary sits between Encode and Layout — the Hamming frame
crosses this boundary carrying only codebook references, not the full
component tree.

## Data Flow

### Server → Client (UI Rendering)

```
1. Agent generates UI intent (e.g., "show a card with title and button")
2. Hamming Encoder resolves to codebook refs:
   [0x020, 0x082, 0x080, 0x081, 0x401]
   (createSurface, Card, Text, Button, theme:dark)
3. Frame Builder packs into Mode B frame (32B header + 10B payload + 16B HMAC = 58 bytes)
   Compare: equivalent JSON would be ~500-2000 bytes
4. Frame sent via UDP (low latency) or gRPC (reliable)
5. Client WASM decodes frame, resolves codebook refs
6. Renderer applies operations to surface
```

### Client → Server (User Actions)

```
1. User clicks a button
2. Client encodes as XOR composition: CLICK ⊕ BUTTON
   Mode C frame: [0x800, 0x081] = 32B header + 3B payload + 16B HMAC = 51 bytes
3. Server decodes, routes to agent for handling
```

### Codebook Sync

```
1. Client connects with codebook_version=0 (first time)
2. Server sends full codebook via Arrow Flight DoGet (4096 × 1250B ≈ 5MB)
   OR chunked XOR stream via gRPC (same data, smaller chunks)
3. Subsequent reconnects: client sends codebook_version=N
4. Server computes XOR delta (N → current), sends only changed entries
   Typical delta: 5-50 entries × 1252B = 6-62KB
```

### Cognitive Feedback Loop

```
1. Usage statistics collected: which codebook refs are frequently co-used
2. Stats sent to ladybug-rs cognitive layer (NARS inference)
3. NARS identifies pattern: "Card+Text+Button always appear together"
4. Proposes composite: codebook[0xC00] = Card ⊕ Text ⊕ Button
5. Delta pushed to clients: one new entry in Dynamic zone
6. Future frames use single ref 0xC00 instead of three refs
   Frame shrinks from 58 bytes to 54 bytes (and more importantly,
   single ref = single atomic operation = faster rendering)
```

## Compression Characteristics

| Scenario | JSON Size | Hamming Size | Ratio |
|----------|-----------|-------------|-------|
| createSurface (simple) | ~500B | 58B | 8.6× |
| updateComponents (5 items) | ~2KB | 64B | 31× |
| Full page (20 components) | ~10KB | 96B | 104× |
| User click action | ~200B | 51B | 3.9× |
| After cognitive optimization | — | even smaller | — |

## Building

### Rust Library

```bash
cd packages/hamming/rust

# Default build (SIMD + gRPC)
cargo build --release

# With Arrow Flight support
cargo build --release --features flight

# WASM build (requires wasm-pack)
wasm-pack build --target web --features wasm --no-default-features
```

### TypeScript Wrapper

```bash
cd packages/hamming/typescript
npm install
npm run build
```

### Protobuf Generation

Protobuf codegen happens automatically via `build.rs` when the `grpc`
feature is enabled. Generated code goes to `src/generated/`.

## Feature Flags

| Flag | Default | Description |
|------|---------|-------------|
| `simd` | yes | SIMD-accelerated Hamming distance (x86_64 POPCNT) |
| `grpc` | yes | tonic gRPC server/client |
| `flight` | no | Arrow Flight integration (for ladybug-rs) |
| `wasm` | no | Browser WASM bindings |
| `cognitive` | no | ladybug-rs cognitive codebook integration |
| `firefly` | no | firefly GEL execution graph integration |
| `full` | no | All features |

## Security Model

- **Session auth**: Bearer token in gRPC InitSession
- **Frame auth**: HMAC-SHA256 (truncated to 16B) per UDP frame
- **Codebook integrity**: Cryptographic chain hashes across versions
- **XOR delta integrity**: Target hash verified after applying deltas
- **Key derivation**: Per-session HMAC keys derived from server secret + timestamp

## Compatibility

- **A2UI v0.8/v0.9**: Codebook entries tagged with spec version
- **JSON fallback**: Clients negotiate JSON mode during handshake
- **ladybug-rs**: Same fingerprint generation, same HammingVector layout
- **firefly**: Same HammingVector (157 × u64), same XOR binding semantics
