/**
 * A2UI Hamming — Content Script.
 *
 * Injected into web pages. Responsibilities:
 * 1. Report page context (URL, title, selection) to the service worker
 * 2. Inject A2UI surface overlays when requested by the agent
 * 3. Bridge between page DOM events and the extension
 */

import {
  type ExtensionMessage,
  isMessage,
  sendToBackground,
} from '../messages.js';

// ── Page context reporting ───────────────────────────────────

// Report page context on load
sendToBackground({
  type: 'page-context',
  url: window.location.href,
  title: document.title,
});

// Report selection changes (debounced)
let selectionTimer: ReturnType<typeof setTimeout> | null = null;

document.addEventListener('selectionchange', () => {
  if (selectionTimer) clearTimeout(selectionTimer);
  selectionTimer = setTimeout(() => {
    const selection = window.getSelection()?.toString().trim();
    if (selection && selection.length > 0 && selection.length < 5000) {
      sendToBackground({
        type: 'page-context',
        url: window.location.href,
        title: document.title,
        selection,
      });
    }
  }, 500);
});

// ── Surface injection ────────────────────────────────────────

chrome.runtime.onMessage.addListener((msg: ExtensionMessage) => {
  if (msg.type === 'inject-surface') {
    injectSurface(msg.surfaceId, msg.position, msg.anchorSelector);
  }
});

/** Active injected surfaces. */
const injectedSurfaces = new Map<number, HTMLElement>();

function injectSurface(
  surfaceId: number,
  position: 'sidebar' | 'overlay' | 'inline',
  anchorSelector?: string
): void {
  // Remove if already exists
  removeSurface(surfaceId);

  const container = document.createElement('div');
  container.id = `a2ui-surface-${surfaceId}`;
  container.setAttribute('data-a2ui-surface', String(surfaceId));

  // Shadow DOM for style isolation
  const shadow = container.attachShadow({ mode: 'open' });
  shadow.innerHTML = `
    <style>
      :host {
        all: initial;
        display: block;
        font-family: -apple-system, system-ui, sans-serif;
        font-size: 14px;
        color: #e0e0e0;
      }
      .a2ui-frame {
        background: #1a1a1a;
        border: 1px solid #2a2a2a;
        border-radius: 8px;
        box-shadow: 0 8px 32px rgba(0,0,0,0.6);
        overflow: hidden;
      }
      .a2ui-frame-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 8px 12px;
        background: #111;
        border-bottom: 1px solid #2a2a2a;
        font-size: 11px;
        color: #808080;
        cursor: move;
        user-select: none;
      }
      .a2ui-frame-header .close {
        cursor: pointer;
        color: #808080;
        font-size: 16px;
        line-height: 1;
      }
      .a2ui-frame-header .close:hover { color: #ff4a4a; }
      .a2ui-frame-body {
        padding: 12px;
        min-height: 80px;
        max-height: 400px;
        overflow-y: auto;
      }
      .a2ui-frame-body .placeholder {
        color: #808080;
        font-size: 12px;
        text-align: center;
        padding: 20px;
      }
    </style>
    <div class="a2ui-frame">
      <div class="a2ui-frame-header">
        <span>A2UI surface:${surfaceId}</span>
        <span class="close" id="close">&times;</span>
      </div>
      <div class="a2ui-frame-body">
        <div class="placeholder">Waiting for agent...</div>
      </div>
    </div>
  `;

  // Close button
  shadow.getElementById('close')?.addEventListener('click', () => {
    removeSurface(surfaceId);
  });

  // Position the container
  switch (position) {
    case 'sidebar':
      Object.assign(container.style, {
        position: 'fixed',
        top: '0',
        right: '0',
        width: '360px',
        height: '100vh',
        zIndex: '2147483647',
      });
      break;

    case 'overlay':
      Object.assign(container.style, {
        position: 'fixed',
        bottom: '20px',
        right: '20px',
        width: '340px',
        zIndex: '2147483647',
      });
      break;

    case 'inline':
      if (anchorSelector) {
        const anchor = document.querySelector(anchorSelector);
        if (anchor) {
          anchor.insertAdjacentElement('afterend', container);
          injectedSurfaces.set(surfaceId, container);
          return;
        }
      }
      // Fallback to overlay
      Object.assign(container.style, {
        position: 'fixed',
        bottom: '20px',
        right: '20px',
        width: '340px',
        zIndex: '2147483647',
      });
      break;
  }

  document.body.appendChild(container);
  injectedSurfaces.set(surfaceId, container);

  // Make draggable (overlay/sidebar)
  if (position !== 'inline') {
    makeDraggable(container, shadow.querySelector('.a2ui-frame-header')!);
  }
}

function removeSurface(surfaceId: number): void {
  const existing = injectedSurfaces.get(surfaceId);
  if (existing) {
    existing.remove();
    injectedSurfaces.delete(surfaceId);
  }
}

function makeDraggable(element: HTMLElement, handle: Element): void {
  let isDragging = false;
  let startX = 0;
  let startY = 0;
  let origX = 0;
  let origY = 0;

  handle.addEventListener('mousedown', ((e: Event) => {
    const me = e as MouseEvent;
    isDragging = true;
    startX = me.clientX;
    startY = me.clientY;
    const rect = element.getBoundingClientRect();
    origX = rect.left;
    origY = rect.top;
    me.preventDefault();
  }) as EventListener);

  document.addEventListener('mousemove', (e: MouseEvent) => {
    if (!isDragging) return;
    const dx = e.clientX - startX;
    const dy = e.clientY - startY;
    element.style.left = `${origX + dx}px`;
    element.style.top = `${origY + dy}px`;
    element.style.right = 'auto';
    element.style.bottom = 'auto';
  });

  document.addEventListener('mouseup', () => {
    isDragging = false;
  });
}
