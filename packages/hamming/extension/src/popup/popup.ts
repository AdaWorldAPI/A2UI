/**
 * A2UI Hamming — Popup UI.
 *
 * Quick status view and connection control. Opens the side panel
 * for full rendering.
 */

import {
  type ExtensionMessage,
  type ClientStatus,
  isMessage,
  sendToBackground,
} from '../messages.js';

// ── DOM ──────────────────────────────────────────────────────

const statusEl = document.getElementById('status')!;
const sessionEl = document.getElementById('session')!;
const uptimeEl = document.getElementById('uptime')!;
const cbVersionEl = document.getElementById('cb-version')!;
const cbEntriesEl = document.getElementById('cb-entries')!;
const framesRxEl = document.getElementById('frames-rx')!;
const framesTxEl = document.getElementById('frames-tx')!;
const endpointEl = document.getElementById('endpoint') as HTMLInputElement;
const btnConnect = document.getElementById('btn-connect') as HTMLButtonElement;
const btnPanel = document.getElementById('btn-panel') as HTMLButtonElement;

// ── State ────────────────────────────────────────────────────

let isConnected = false;

// ── Events ───────────────────────────────────────────────────

btnConnect.addEventListener('click', async () => {
  if (isConnected) {
    await sendToBackground({ type: 'disconnect' });
  } else {
    const endpoint = endpointEl.value.trim();
    if (!endpoint) return;
    await sendToBackground({ type: 'connect', endpoint });
  }
});

btnPanel.addEventListener('click', async () => {
  // Open side panel in the current window
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (tab?.id) {
    await chrome.sidePanel.open({ tabId: tab.id });
    window.close();
  }
});

// ── Status updates ───────────────────────────────────────────

chrome.runtime.onMessage.addListener((msg: ExtensionMessage) => {
  if (msg.type === 'connection-state') {
    updateStatus(msg.state);
  }
  if (msg.type === 'status-response') {
    applyFullStatus(msg.status);
  }
});

function updateStatus(state: string): void {
  statusEl.textContent = state;
  statusEl.className = 'value ' + state;
  isConnected = state === 'connected';
  btnConnect.textContent = isConnected ? 'Disconnect' : 'Connect';
  btnConnect.className = isConnected ? 'btn-secondary' : 'btn-primary';
  endpointEl.disabled = isConnected;
}

function applyFullStatus(s: ClientStatus): void {
  updateStatus(s.connectionState);
  sessionEl.textContent = s.sessionId ?? '—';
  uptimeEl.textContent = s.uptime > 0 ? formatUptime(s.uptime) : '—';
  cbVersionEl.textContent = String(s.codebookVersion);
  cbEntriesEl.textContent = String(s.codebookEntries);
  framesRxEl.textContent = String(s.framesReceived);
  framesTxEl.textContent = String(s.framesSent);
  if (s.endpoint) {
    endpointEl.value = s.endpoint;
  }
}

function formatUptime(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return `${h}h ${m}m`;
}

// ── Initial load ─────────────────────────────────────────────

sendToBackground({ type: 'status-request' });

// Poll for updates while popup is open
const poll = setInterval(() => {
  sendToBackground({ type: 'status-request' });
}, 2000);

window.addEventListener('unload', () => clearInterval(poll));
