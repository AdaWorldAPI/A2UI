/**
 * A2UI Hamming — Chrome Extension Service Worker.
 *
 * Central coordinator that:
 * 1. Maintains the WebSocket connection to the agent server
 * 2. Routes raw frames to the offscreen document for WASM decoding
 * 3. Forwards decoded operations to the side panel for rendering
 * 4. Caches the codebook in chrome.storage.local
 * 5. Handles user actions from the side panel / content script
 */

import {
  type ExtensionMessage,
  type ConnectionState,
  type ClientStatus,
  type DecodedOperationDto,
  isMessage,
  sendToTab,
} from '../messages.js';

// ── State ────────────────────────────────────────────────────

let connectionState: ConnectionState = 'disconnected';
let endpoint: string | null = null;
let sessionId: Uint8Array | null = null;
let hmacKey: Uint8Array | null = null;
let ws: WebSocket | null = null;
let codebookVersion = 0;
let codebookEntries = 0;
let framesReceived = 0;
let framesSent = 0;
let lastError: string | null = null;
let connectedAt = 0;
let offscreenReady = false;
let hasOffscreen = false;

// ── Lifecycle ────────────────────────────────────────────────

// Open side panel on extension icon click
chrome.sidePanel
  .setPanelBehavior({ openPanelOnActionClick: true })
  .catch(() => {
    // sidePanel API might not be available in all contexts
  });

// Context menu for page-level interaction
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: 'a2ui-send-selection',
    title: 'Send to A2UI Agent',
    contexts: ['selection'],
  });

  chrome.contextMenus.create({
    id: 'a2ui-inject-surface',
    title: 'Open A2UI Surface Here',
    contexts: ['page'],
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (!tab?.id) return;

  if (info.menuItemId === 'a2ui-send-selection' && info.selectionText) {
    sendToTab(tab.id, {
      type: 'page-context',
      url: tab.url ?? '',
      title: tab.title ?? '',
      selection: info.selectionText,
    });
  }

  if (info.menuItemId === 'a2ui-inject-surface') {
    sendToTab(tab.id, {
      type: 'inject-surface',
      surfaceId: Date.now() & 0xffffffff,
      position: 'overlay',
    });
  }
});

// ── Message handling ─────────────────────────────────────────

chrome.runtime.onMessage.addListener(
  (msg: ExtensionMessage, _sender, sendResponse) => {
    handleMessage(msg, sendResponse);
    return true; // async response
  }
);

async function handleMessage(
  msg: ExtensionMessage,
  sendResponse: (response?: ExtensionMessage) => void
): Promise<void> {
  switch (msg.type) {
    case 'connect':
      await connect(msg.endpoint);
      sendResponse({ type: 'connection-state', state: connectionState });
      break;

    case 'disconnect':
      disconnect();
      sendResponse({ type: 'connection-state', state: connectionState });
      break;

    case 'status-request':
      sendResponse({
        type: 'status-response',
        status: getStatus(),
      });
      break;

    case 'frame-decoded':
      // From offscreen → route to side panel
      framesReceived++;
      broadcastToAll({
        type: 'render-operation',
        operation: msg.operation,
      });
      break;

    case 'wasm-ready':
      offscreenReady = true;
      codebookVersion = msg.codebookVersion;
      codebookEntries = msg.entryCount;
      break;

    case 'delta-applied':
      codebookVersion = msg.newVersion;
      break;

    case 'action-frame-built':
      // From offscreen → send over WebSocket
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(new Uint8Array(msg.frameData));
        framesSent++;
      }
      break;

    case 'page-context':
      // From content script → could trigger agent interaction
      // For now, forward to side panel as context
      broadcastToAll(msg);
      break;

    case 'build-action-frame':
      // From side panel → route to offscreen for frame building
      await ensureOffscreen();
      chrome.runtime.sendMessage(msg);
      break;

    default:
      break;
  }
}

// ── Connection management ────────────────────────────────────

async function connect(url: string): Promise<void> {
  if (ws) {
    disconnect();
  }

  endpoint = url;
  setState('connecting');

  try {
    await ensureOffscreen();

    // Generate session credentials
    sessionId = crypto.getRandomValues(new Uint8Array(8));
    hmacKey = crypto.getRandomValues(new Uint8Array(32));

    // Initialize WASM with cached codebook if available
    const cached = await loadCachedCodebook();
    chrome.runtime.sendMessage({
      type: 'init-wasm',
      sessionId: Array.from(sessionId),
      hmacKey: Array.from(hmacKey),
      codebookData: cached ? Array.from(cached) : undefined,
    } satisfies ExtensionMessage);

    // Connect WebSocket
    ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      setState('handshake');
      sendHandshake();
    };

    ws.onmessage = (event: MessageEvent) => {
      handleFrame(event.data as ArrayBuffer);
    };

    ws.onerror = () => {
      lastError = 'WebSocket connection failed';
      setState('error');
    };

    ws.onclose = (event: CloseEvent) => {
      const wasConnected = connectionState === 'connected';
      setState('disconnected');

      // Auto-reconnect if unexpected closure
      if (wasConnected && !event.wasClean && endpoint) {
        scheduleReconnect();
      }
    };
  } catch (err) {
    lastError = err instanceof Error ? err.message : String(err);
    setState('error');
  }
}

function disconnect(): void {
  if (ws) {
    ws.close(1000, 'user-disconnect');
    ws = null;
  }
  setState('disconnected');
  endpoint = null;
  sessionId = null;
  hmacKey = null;
}

function sendHandshake(): void {
  if (!ws || !sessionId) return;

  // Handshake frame: Mode B with HANDSHAKE_INIT ref
  // For the prototype, send a JSON handshake that the server can parse
  // to negotiate capabilities. Production would use a Mode B Hamming frame.
  const handshake = {
    protocol: 'a2ui-hamming',
    version: 1,
    sessionId: Array.from(sessionId),
    codebookVersion,
    capabilities: {
      modes: ['A', 'B', 'C', 'D'],
      compression: ['lz4'],
      maxFrameSize: 1472,
      wasmDecode: true,
    },
  };

  ws.send(JSON.stringify(handshake));
  setState('syncing-codebook');

  // After handshake, transition to connected
  // (In production, server responds with codebook delta first)
  setTimeout(() => {
    if (connectionState === 'syncing-codebook') {
      connectedAt = Date.now();
      setState('connected');
    }
  }, 500);
}

function handleFrame(data: ArrayBuffer): void {
  if (!offscreenReady) return;

  const bytes = new Uint8Array(data);

  // Check if this is a JSON control message (handshake response)
  if (bytes[0] === 0x7b) {
    // '{' — JSON
    try {
      const msg = JSON.parse(new TextDecoder().decode(bytes));
      handleControlMessage(msg);
      return;
    } catch {
      // Not JSON, treat as Hamming frame
    }
  }

  // Route binary frame to offscreen WASM for decoding
  chrome.runtime.sendMessage({
    type: 'decode-frame',
    frameData: Array.from(bytes),
  } satisfies ExtensionMessage);
}

function handleControlMessage(msg: Record<string, unknown>): void {
  if (msg.type === 'codebook-delta') {
    // Apply codebook delta
    const deltaData = msg.data as number[];
    chrome.runtime.sendMessage({
      type: 'apply-delta',
      deltaData,
    } satisfies ExtensionMessage);

    // Cache updated codebook
    cacheCodebook(new Uint8Array(deltaData));
  }

  if (msg.type === 'session-established') {
    connectedAt = Date.now();
    setState('connected');
  }
}

// ── Reconnection ─────────────────────────────────────────────

let reconnectAttempt = 0;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleReconnect(): void {
  if (reconnectAttempt >= 4) {
    lastError = 'Max reconnection attempts reached';
    setState('error');
    return;
  }

  const delay = Math.pow(2, reconnectAttempt + 1) * 1000; // 2s, 4s, 8s, 16s
  reconnectAttempt++;

  reconnectTimer = setTimeout(() => {
    if (endpoint) {
      connect(endpoint);
    }
  }, delay);
}

function resetReconnect(): void {
  reconnectAttempt = 0;
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
}

// ── Offscreen document management ────────────────────────────

async function ensureOffscreen(): Promise<void> {
  if (hasOffscreen) return;

  // Check if already exists (service worker might have restarted)
  const contexts = await chrome.runtime.getContexts({
    contextTypes: [chrome.runtime.ContextType.OFFSCREEN_DOCUMENT],
  });

  if (contexts.length > 0) {
    hasOffscreen = true;
    return;
  }

  await chrome.offscreen.createDocument({
    url: 'offscreen/index.html',
    reasons: [chrome.offscreen.Reason.WORKERS],
    justification: 'WASM Hamming frame decoding',
  });

  hasOffscreen = true;
}

// ── Codebook cache ───────────────────────────────────────────

const CODEBOOK_CACHE_KEY = 'a2ui-codebook';
const CODEBOOK_VERSION_KEY = 'a2ui-codebook-version';

async function loadCachedCodebook(): Promise<Uint8Array | null> {
  try {
    const result = await chrome.storage.local.get([
      CODEBOOK_CACHE_KEY,
      CODEBOOK_VERSION_KEY,
    ]);
    if (result[CODEBOOK_CACHE_KEY]) {
      codebookVersion = result[CODEBOOK_VERSION_KEY] ?? 0;
      return new Uint8Array(result[CODEBOOK_CACHE_KEY]);
    }
  } catch {
    // Cache miss
  }
  return null;
}

async function cacheCodebook(data: Uint8Array): Promise<void> {
  try {
    await chrome.storage.local.set({
      [CODEBOOK_CACHE_KEY]: Array.from(data),
      [CODEBOOK_VERSION_KEY]: codebookVersion,
    });
  } catch {
    // Storage full or unavailable — non-fatal
  }
}

// ── State broadcasting ───────────────────────────────────────

function setState(newState: ConnectionState): void {
  connectionState = newState;
  if (newState === 'connected') {
    resetReconnect();
  }
  broadcastToAll({ type: 'connection-state', state: newState });
}

function broadcastToAll(msg: ExtensionMessage): void {
  // Broadcast to all extension contexts (side panel, popup, etc.)
  chrome.runtime.sendMessage(msg).catch(() => {
    // No listeners — that's fine
  });
}

function getStatus(): ClientStatus {
  return {
    connectionState,
    endpoint,
    sessionId: sessionId ? Array.from(sessionId).map(b => b.toString(16).padStart(2, '0')).join('') : null,
    codebookVersion,
    codebookEntries,
    framesReceived,
    framesSent,
    lastError,
    uptime: connectedAt > 0 ? Math.floor((Date.now() - connectedAt) / 1000) : 0,
  };
}
