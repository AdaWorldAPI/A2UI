/**
 * A2UI Hamming — PWA Standalone App.
 *
 * Full-viewport thin client for ChromeOS / standalone deployment.
 * Same WASM core as the Chrome extension, but runs directly in the
 * page without extension APIs.
 *
 * Connects via WebSocket and decodes Hamming frames in-page.
 */

import { HammingTransport, type DecodedOperation, Addresses } from '../../typescript/src/index.js';

// ── DOM ──────────────────────────────────────────────────────

const statusDot = document.getElementById('status-dot')!;
const statusLabel = document.getElementById('status-label')!;
const surfacesEl = document.getElementById('surfaces')!;
const endpointEl = document.getElementById('endpoint') as HTMLInputElement;
const connectBtn = document.getElementById('connect-btn') as HTMLButtonElement;

// ── State ────────────────────────────────────────────────────

let transport: HammingTransport | null = null;
let connected = false;
const surfaces = new Map<number, HTMLElement>();

// ── Connection ───────────────────────────────────────────────

connectBtn.addEventListener('click', async () => {
  if (connected && transport) {
    transport.disconnect();
    setStatus('disconnected');
    connected = false;
    connectBtn.textContent = 'Connect';
    endpointEl.disabled = false;
    return;
  }

  const endpoint = endpointEl.value.trim();
  if (!endpoint) return;

  setStatus('connecting');
  connectBtn.textContent = 'Cancel';
  endpointEl.disabled = true;

  try {
    // Load WASM module
    const wasmModule = await loadWasmModule();

    transport = await HammingTransport.connect(endpoint, { wasmModule });

    transport.onOperation(handleOperation);
    transport.onError((err) => {
      log(`Error: ${err.message}`);
      setStatus('error');
    });

    connected = true;
    setStatus('connected');
    connectBtn.textContent = 'Disconnect';
    log(`Connected to ${endpoint}`);
  } catch (err) {
    log(`Connection failed: ${err instanceof Error ? err.message : err}`);
    setStatus('error');
    connectBtn.textContent = 'Connect';
    endpointEl.disabled = false;
  }
});

// ── WASM loading ─────────────────────────────────────────────

async function loadWasmModule(): Promise<any> {
  try {
    // Try loading the built WASM module
    const module = await import(/* webpackIgnore: true */ '../wasm/a2ui_hamming.js');
    await module.default();
    return module;
  } catch {
    // Return a stub that uses the JS fallback decoder
    log('WASM not available, using JS fallback');
    return createFallbackModule();
  }
}

function createFallbackModule(): any {
  const MAGIC = 0xa2b1;
  const HEADER_SIZE = 32;

  class WasmHammingClient {
    constructor(_sessionId: Uint8Array, _hmacKey: Uint8Array) {}

    codebookVersion() { return 1; }
    codebookEntryCount() { return 0; }

    decodeFrame(data: Uint8Array): DecodedOperation {
      if (data.length < HEADER_SIZE) {
        return { op_type: 'raw', surface_id: 0, details: {} };
      }
      const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
      const flags = view.getUint8(3);
      const mode = (flags >> 6) & 0x03;
      const surfaceId = view.getUint32(16, true);
      const opCount = view.getUint16(20, true);

      if (mode === 1 && opCount > 0) {
        const firstRef = view.getUint16(HEADER_SIZE, true) >> 4;
        const opType = ({
          [Addresses.CREATE_SURFACE]: 'createSurface',
          [Addresses.UPDATE_COMPONENTS]: 'updateComponents',
          [Addresses.UPDATE_DATA_MODEL]: 'updateDataModel',
          [Addresses.DELETE_SURFACE]: 'deleteSurface',
        } as Record<number, string>)[firstRef] ?? 'raw';

        return {
          op_type: opType as DecodedOperation['op_type'],
          surface_id: surfaceId,
          details: { mode: 'B', opCount, firstRef },
        };
      }
      return { op_type: 'raw', surface_id: surfaceId, details: { mode, opCount } };
    }

    buildXorFrame() { return new Uint8Array(0); }
    buildRefFrame() { return new Uint8Array(0); }
    lookupAddress() { return null; }
    codebookStats() { return { version: 1, entry_count: 0, zones: [] }; }
  }

  return { WasmHammingClient };
}

// ── Operation handling ───────────────────────────────────────

function handleOperation(op: DecodedOperation): void {
  const surfaceId = op.surface_id;

  switch (op.op_type) {
    case 'createSurface': {
      removeSurface(surfaceId);
      const el = document.createElement('div');
      el.className = 'a2ui-surface';
      el.dataset.sid = String(surfaceId);
      el.innerHTML = `
        <div style="
          border: 1px solid #2a2a2a;
          border-radius: 8px;
          background: #1a1a1a;
          margin-bottom: 12px;
          overflow: hidden;
        ">
          <div style="
            padding: 8px 12px;
            background: #111;
            border-bottom: 1px solid #2a2a2a;
            font-size: 11px;
            color: #808080;
          ">surface:${surfaceId}</div>
          <div class="surface-content" style="padding: 12px;">
            ${renderDetails(op.details)}
          </div>
        </div>
      `;
      surfacesEl.appendChild(el);
      surfaces.set(surfaceId, el);
      log(`Surface ${surfaceId} created`);
      break;
    }
    case 'updateComponents':
    case 'updateDataModel': {
      const existing = surfaces.get(surfaceId);
      if (existing) {
        const content = existing.querySelector('.surface-content');
        if (content) content.innerHTML += renderDetails(op.details);
      }
      log(`Surface ${surfaceId} updated`);
      break;
    }
    case 'deleteSurface':
      removeSurface(surfaceId);
      log(`Surface ${surfaceId} deleted`);
      break;
    default:
      log(`Op: ${op.op_type} [${surfaceId}]`);
  }
}

function removeSurface(surfaceId: number): void {
  const el = surfaces.get(surfaceId);
  if (el) {
    el.remove();
    surfaces.delete(surfaceId);
  }
}

function renderDetails(details: Record<string, unknown>): string {
  return `<pre style="font-size: 11px; color: #808080; white-space: pre-wrap;">${JSON.stringify(details, null, 2)}</pre>`;
}

// ── Status UI ────────────────────────────────────────────────

function setStatus(state: 'disconnected' | 'connecting' | 'connected' | 'error'): void {
  statusDot.className = 'dot ' + state;
  statusLabel.textContent = state.charAt(0).toUpperCase() + state.slice(1);
}

function log(msg: string): void {
  console.log(`[A2UI] ${msg}`);
}

// ── Service Worker registration (offline support) ────────────

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').catch(() => {
    // Service worker not available — that's fine for prototype
  });
}
