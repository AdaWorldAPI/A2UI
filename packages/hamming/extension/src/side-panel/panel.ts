/**
 * A2UI Hamming — Side Panel renderer.
 *
 * Receives decoded operations from the service worker and renders
 * A2UI surfaces. In production this would import the Lit renderer
 * components directly. For the prototype we render a structured
 * operation log and basic surface visualization.
 */

import {
  type ExtensionMessage,
  type ConnectionState,
  type ClientStatus,
  type DecodedOperationDto,
  isMessage,
  sendToBackground,
} from '../messages.js';

// ── DOM references ───────────────────────────────────────────

const statusIndicator = document.getElementById('status-indicator')!;
const statusText = document.getElementById('status-text')!;
const statusMeta = document.getElementById('status-meta')!;
const connectForm = document.getElementById('connect-form') as HTMLFormElement;
const endpointInput = document.getElementById('endpoint-input') as HTMLInputElement;
const connectBtn = document.getElementById('connect-btn') as HTMLButtonElement;
const surfacesContainer = document.getElementById('surfaces')!;
const logContainer = document.getElementById('log')!;

// ── State ────────────────────────────────────────────────────

let connected = false;
const surfaces = new Map<number, HTMLElement>();
let opCount = 0;

// ── UI setup ─────────────────────────────────────────────────

connectForm.addEventListener('submit', async (e) => {
  e.preventDefault();
  if (connected) {
    await sendToBackground({ type: 'disconnect' });
  } else {
    const endpoint = endpointInput.value.trim();
    if (!endpoint) return;
    await sendToBackground({ type: 'connect', endpoint });
  }
});

// Request initial status
sendToBackground({ type: 'status-request' });

// ── Message handling ─────────────────────────────────────────

chrome.runtime.onMessage.addListener((msg: ExtensionMessage) => {
  switch (msg.type) {
    case 'connection-state':
      updateConnectionUI(msg.state);
      break;

    case 'render-operation':
      renderOperation(msg.operation);
      break;

    case 'status-response':
      applyStatus(msg.status);
      break;

    case 'page-context':
      log(`Page context: ${msg.title} (${msg.url})${msg.selection ? ` — "${msg.selection}"` : ''}`);
      break;
  }
});

// ── Connection UI ────────────────────────────────────────────

function updateConnectionUI(state: ConnectionState): void {
  statusIndicator.className = 'indicator';
  connected = state === 'connected';

  switch (state) {
    case 'disconnected':
      statusText.textContent = 'Disconnected';
      statusMeta.textContent = '';
      connectBtn.textContent = 'Connect';
      connectBtn.className = '';
      endpointInput.disabled = false;
      break;

    case 'connecting':
      statusIndicator.classList.add('connecting');
      statusText.textContent = 'Connecting...';
      connectBtn.textContent = 'Cancel';
      connectBtn.className = 'disconnect';
      endpointInput.disabled = true;
      break;

    case 'handshake':
      statusIndicator.classList.add('connecting');
      statusText.textContent = 'Handshake...';
      break;

    case 'syncing-codebook':
      statusIndicator.classList.add('connecting');
      statusText.textContent = 'Syncing codebook...';
      break;

    case 'connected':
      statusIndicator.classList.add('connected');
      statusText.textContent = 'Connected';
      connectBtn.textContent = 'Disconnect';
      connectBtn.className = 'disconnect';
      endpointInput.disabled = true;
      log('Connected to agent', 'op');
      break;

    case 'error':
      statusIndicator.classList.add('error');
      statusText.textContent = 'Error';
      connectBtn.textContent = 'Retry';
      connectBtn.className = '';
      endpointInput.disabled = false;
      break;
  }
}

function applyStatus(status: ClientStatus): void {
  updateConnectionUI(status.connectionState);
  if (status.endpoint) {
    endpointInput.value = status.endpoint;
  }
  if (status.codebookVersion > 0) {
    statusMeta.textContent = `cb:v${status.codebookVersion} (${status.codebookEntries}) | rx:${status.framesReceived} tx:${status.framesSent}`;
  }
  if (status.lastError) {
    log(`Error: ${status.lastError}`, 'error');
  }
}

// ── Surface rendering ────────────────────────────────────────

function renderOperation(op: DecodedOperationDto): void {
  opCount++;
  statusMeta.textContent = `ops: ${opCount}`;

  switch (op.opType) {
    case 'createSurface':
      createSurface(op.surfaceId, op.details);
      break;
    case 'updateComponents':
      updateSurface(op.surfaceId, op.details);
      break;
    case 'updateDataModel':
      updateSurface(op.surfaceId, op.details);
      break;
    case 'deleteSurface':
      deleteSurface(op.surfaceId);
      break;
    case 'userAction':
      log(`Action on surface ${op.surfaceId}: ${JSON.stringify(op.details)}`, 'op');
      break;
    case 'cognitiveOp':
      log(`Cognitive: ${JSON.stringify(op.details)}`, 'op');
      break;
    default:
      log(`Raw op [${op.surfaceId}]: ${JSON.stringify(op.details)}`);
      break;
  }
}

function createSurface(
  surfaceId: number,
  details: Record<string, unknown>
): void {
  // Remove existing if present
  deleteSurface(surfaceId);

  const surface = document.createElement('div');
  surface.className = 'surface';
  surface.dataset.surfaceId = String(surfaceId);

  const header = document.createElement('div');
  header.className = 'surface-header';
  header.innerHTML = `
    <span>surface:${surfaceId}</span>
    <span>${details.root_component ? `root: ${(details.root_component as Record<string, unknown>).component_type ?? 'unknown'}` : ''}</span>
    <span>${details.theme ? `theme: ${(details.theme as Record<string, unknown>).style_type ?? ''}` : ''}</span>
  `;

  const body = document.createElement('div');
  body.className = 'surface-body';

  // In production, this would mount Lit A2UI renderer components.
  // For the prototype, render a structured representation.
  body.innerHTML = renderComponentTree(details);

  surface.appendChild(header);
  surface.appendChild(body);
  surfacesContainer.appendChild(surface);
  surfaces.set(surfaceId, surface);

  log(`Surface ${surfaceId} created`, 'op');
}

function updateSurface(
  surfaceId: number,
  details: Record<string, unknown>
): void {
  const surface = surfaces.get(surfaceId);
  if (!surface) {
    // Surface not yet created — create it
    createSurface(surfaceId, details);
    return;
  }

  const body = surface.querySelector('.surface-body');
  if (body) {
    // Append new components to existing surface
    body.innerHTML += renderComponentTree(details);
  }

  log(`Surface ${surfaceId} updated`, 'op');
}

function deleteSurface(surfaceId: number): void {
  const surface = surfaces.get(surfaceId);
  if (surface) {
    surface.remove();
    surfaces.delete(surfaceId);
    log(`Surface ${surfaceId} deleted`);
  }
}

function renderComponentTree(details: Record<string, unknown>): string {
  const parts: string[] = [];

  // Render resolved references as component indicators
  const components =
    (details.components as Array<Record<string, unknown>>) ??
    (details.additional as Array<Record<string, unknown>>) ??
    (details.refs as Array<Record<string, unknown>>) ??
    [];

  for (const comp of components) {
    const label = (comp.semantic_label as string) ?? `0x${((comp.address as number) ?? 0).toString(16).padStart(3, '0')}`;
    const zone = (comp.zone as string) ?? 'unknown';
    const addr = ((comp.address as number) ?? 0).toString(16).padStart(3, '0');

    parts.push(`
      <div style="
        display: inline-flex;
        align-items: center;
        gap: 6px;
        padding: 4px 10px;
        margin: 3px;
        background: var(--a2ui-surface);
        border: 1px solid var(--a2ui-border);
        border-radius: 4px;
        font-size: 11px;
      ">
        <span style="color: var(--a2ui-accent); font-weight: 600;">
          ${shortLabel(label)}
        </span>
        <span style="color: var(--a2ui-text-dim);">
          0x${addr}
        </span>
        <span style="
          padding: 1px 5px;
          background: var(--a2ui-border);
          border-radius: 2px;
          font-size: 10px;
          color: var(--a2ui-text-dim);
        ">
          ${zone}
        </span>
      </div>
    `);
  }

  // Data model refs
  const dataRefs = (details.data_refs as Array<Record<string, unknown>>) ?? [];
  for (const ref of dataRefs) {
    const label = (ref.semantic_label as string) ?? 'data';
    parts.push(`
      <div style="
        display: inline-flex;
        align-items: center;
        gap: 4px;
        padding: 3px 8px;
        margin: 3px;
        background: #1a1a2a;
        border: 1px solid #2a2a3a;
        border-radius: 4px;
        font-size: 11px;
        color: #8888ff;
      ">
        ${shortLabel(label)}
      </div>
    `);
  }

  return parts.join('');
}

function shortLabel(label: string): string {
  // Strip common prefixes for display
  return label
    .replace('a2ui.component.', '')
    .replace('a2ui.style.', '')
    .replace('a2ui.message.', '')
    .replace('a2ui.action.', '')
    .replace('a2ui.cognitive.', '');
}

// ── Log ──────────────────────────────────────────────────────

function log(message: string, className = ''): void {
  const entry = document.createElement('div');
  entry.className = `entry ${className}`;
  const ts = new Date().toLocaleTimeString('en-US', { hour12: false, fractionalSecondDigits: 0 });
  entry.textContent = `[${ts}] ${message}`;
  logContainer.appendChild(entry);

  // Keep log reasonable
  while (logContainer.children.length > 200) {
    logContainer.firstChild?.remove();
  }

  logContainer.scrollTop = logContainer.scrollHeight;
}

// ── User action helpers ──────────────────────────────────────

/**
 * Send a user action from the side panel to the agent.
 * Called by rendered A2UI components when the user interacts.
 */
(window as any).a2uiAction = (
  surfaceId: number,
  actionAddress: number,
  targetAddresses: number[]
) => {
  sendToBackground({
    type: 'build-action-frame',
    surfaceId,
    actionAddress,
    targetAddresses,
  });
  log(`Action sent: 0x${actionAddress.toString(16)} → [${targetAddresses.map(a => '0x' + a.toString(16)).join(', ')}]`, 'op');
};
