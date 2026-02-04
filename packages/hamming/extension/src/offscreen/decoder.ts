/**
 * A2UI Hamming — Offscreen WASM Decoder.
 *
 * Runs in the offscreen document. Loads the WASM Hamming module,
 * maintains the client-side codebook, decodes incoming frames,
 * and builds outgoing action frames.
 *
 * Communication with the service worker is via chrome.runtime messages.
 */

import {
  type ExtensionMessage,
  type DecodedOperationDto,
  isMessage,
} from '../messages.js';

// ── WASM module handle ───────────────────────────────────────

let wasmClient: any = null;
let initialized = false;

// ── Message handling ─────────────────────────────────────────

chrome.runtime.onMessage.addListener(
  (msg: ExtensionMessage, _sender, sendResponse) => {
    handleMessage(msg, sendResponse);
    return true;
  }
);

async function handleMessage(
  msg: ExtensionMessage,
  sendResponse: (response?: ExtensionMessage) => void
): Promise<void> {
  switch (msg.type) {
    case 'init-wasm':
      await initWasm(
        new Uint8Array(msg.sessionId),
        new Uint8Array(msg.hmacKey),
        msg.codebookData ? new Uint8Array(msg.codebookData) : undefined
      );
      sendResponse({
        type: 'wasm-ready',
        codebookVersion: wasmClient?.codebookVersion() ?? 0,
        entryCount: wasmClient?.codebookEntryCount() ?? 0,
      });
      break;

    case 'decode-frame':
      if (!initialized || !wasmClient) {
        return;
      }
      try {
        const frameBytes = new Uint8Array(msg.frameData);
        const result = wasmClient.decodeFrame(frameBytes);
        const operation = parseWasmResult(result);
        sendResponse({
          type: 'frame-decoded',
          operation,
        });
      } catch (err) {
        console.error('[A2UI Offscreen] Frame decode error:', err);
      }
      break;

    case 'apply-delta':
      if (!initialized || !wasmClient) {
        return;
      }
      try {
        const deltaBytes = new Uint8Array(msg.deltaData);
        wasmClient.applyDelta(deltaBytes);
        sendResponse({
          type: 'delta-applied',
          newVersion: wasmClient.codebookVersion(),
        });
      } catch (err) {
        console.error('[A2UI Offscreen] Delta apply error:', err);
      }
      break;

    case 'build-action-frame':
      if (!initialized || !wasmClient) {
        return;
      }
      try {
        const frame = wasmClient.buildXorFrame(
          msg.surfaceId,
          Date.now() & 0xffffffff, // sequence
          msg.actionAddress,
          new Uint16Array(msg.targetAddresses)
        );
        sendResponse({
          type: 'action-frame-built',
          frameData: Array.from(new Uint8Array(frame)),
        });
      } catch (err) {
        console.error('[A2UI Offscreen] Frame build error:', err);
      }
      break;
  }
}

// ── WASM initialization ──────────────────────────────────────

async function initWasm(
  sessionId: Uint8Array,
  hmacKey: Uint8Array,
  cachedCodebook?: Uint8Array
): Promise<void> {
  try {
    // Load WASM module
    // In production: import init, { WasmHammingClient } from '../../wasm/a2ui_hamming.js';
    // For prototype, we dynamically import from the extension's wasm/ directory
    const wasmUrl = chrome.runtime.getURL('wasm/a2ui_hamming_bg.wasm');
    const wasmModuleUrl = chrome.runtime.getURL('wasm/a2ui_hamming.js');

    const module = await import(/* webpackIgnore: true */ wasmModuleUrl);
    await module.default(wasmUrl);

    if (cachedCodebook && cachedCodebook.length > 0) {
      wasmClient = module.WasmHammingClient.fromCachedCodebook(
        sessionId,
        hmacKey,
        cachedCodebook
      );
    } else {
      wasmClient = new module.WasmHammingClient(sessionId, hmacKey);
    }

    initialized = true;
    console.log(
      '[A2UI Offscreen] WASM initialized, codebook v%d with %d entries',
      wasmClient.codebookVersion(),
      wasmClient.codebookEntryCount()
    );
  } catch (err) {
    console.error('[A2UI Offscreen] WASM init failed:', err);

    // Fallback: create a stub client that uses pure JS decoding
    wasmClient = createFallbackClient(sessionId, hmacKey);
    initialized = true;
    console.log('[A2UI Offscreen] Using JS fallback decoder');
  }
}

// ── Result parsing ───────────────────────────────────────────

function parseWasmResult(result: unknown): DecodedOperationDto {
  // WASM returns a JsValue that serde-wasm-bindgen serializes as a plain object
  if (typeof result === 'object' && result !== null) {
    const obj = result as Record<string, unknown>;
    return {
      opType: (obj.op_type as DecodedOperationDto['opType']) ?? 'raw',
      surfaceId: (obj.surface_id as number) ?? 0,
      details: (obj.details as Record<string, unknown>) ?? {},
    };
  }

  return {
    opType: 'raw',
    surfaceId: 0,
    details: { raw: result },
  };
}

// ── JS fallback decoder ──────────────────────────────────────

/**
 * Pure JS fallback when WASM fails to load. Provides basic frame
 * parsing without the full Hamming codebook resolution.
 */
function createFallbackClient(
  sessionId: Uint8Array,
  hmacKey: Uint8Array
): Record<string, Function> {
  const MAGIC = 0xa2b1;
  const HEADER_SIZE = 32;

  return {
    codebookVersion: () => 1,
    codebookEntryCount: () => 0,

    decodeFrame(data: Uint8Array): DecodedOperationDto {
      if (data.length < HEADER_SIZE) {
        return { opType: 'raw', surfaceId: 0, details: { error: 'frame too short' } };
      }

      const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
      const magic = view.getUint16(0, true);
      if (magic !== MAGIC) {
        return { opType: 'raw', surfaceId: 0, details: { error: 'invalid magic' } };
      }

      const flags = view.getUint8(3);
      const mode = (flags >> 6) & 0x03;
      const surfaceId = view.getUint32(16, true);
      const opCount = view.getUint16(20, true);

      // Mode B: extract codebook references
      if (mode === 1) {
        const refs: number[] = [];
        let offset = HEADER_SIZE;
        for (let i = 0; i < opCount && offset + 1 < data.length - 16; i++) {
          const packed = view.getUint16(offset, true);
          refs.push(packed >> 4); // 12-bit address
          offset += 2;
        }

        return {
          opType: mapMessageType(refs[0]),
          surfaceId,
          details: { addresses: refs, mode: 'B' },
        };
      }

      return {
        opType: 'raw',
        surfaceId,
        details: { mode, opCount, payloadSize: data.length - HEADER_SIZE - 16 },
      };
    },

    buildXorFrame(
      surfaceId: number,
      sequence: number,
      baseAddr: number,
      chain: Uint16Array
    ): Uint8Array {
      // Minimal Mode C frame
      const payloadSize = 2 + chain.length * 2;
      const frame = new Uint8Array(HEADER_SIZE + payloadSize + 16);
      const view = new DataView(frame.buffer);

      view.setUint16(0, MAGIC, true);
      view.setUint8(2, 0x01); // version
      view.setUint8(3, 0x80); // Mode C
      view.setUint32(4, sequence, true);
      view.setUint32(16, surfaceId, true);
      view.setUint16(20, 1 + chain.length, true);

      let offset = HEADER_SIZE;
      view.setUint16(offset, baseAddr << 4, true);
      offset += 2;
      for (let i = 0; i < chain.length; i++) {
        view.setUint16(offset, chain[i] << 4, true);
        offset += 2;
      }

      return frame;
    },

    applyDelta(_data: Uint8Array): void {
      // No-op in fallback mode
    },
  };
}

function mapMessageType(address?: number): DecodedOperationDto['opType'] {
  switch (address) {
    case 0x020: return 'createSurface';
    case 0x021: return 'updateComponents';
    case 0x022: return 'updateDataModel';
    case 0x023: return 'deleteSurface';
    default: return 'raw';
  }
}
