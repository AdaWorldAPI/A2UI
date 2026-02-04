/**
 * Extension messaging protocol.
 *
 * All communication between contexts (service worker, side panel, offscreen,
 * content script, popup) goes through chrome.runtime message passing.
 * This module defines the typed message schema and dispatch helpers.
 */

// ── Message types ────────────────────────────────────────────

/** Service worker → offscreen: decode this raw frame */
export interface DecodeFrameMsg {
  type: 'decode-frame';
  frameData: number[]; // Uint8Array serialized as number[]
}

/** Offscreen → service worker: decoded operation result */
export interface FrameDecodedMsg {
  type: 'frame-decoded';
  operation: DecodedOperationDto;
}

/** Service worker → side panel: render this operation */
export interface RenderOperationMsg {
  type: 'render-operation';
  operation: DecodedOperationDto;
}

/** Popup / side panel → service worker: connect to an agent endpoint */
export interface ConnectMsg {
  type: 'connect';
  endpoint: string;
}

/** Popup / side panel → service worker: disconnect */
export interface DisconnectMsg {
  type: 'disconnect';
}

/** Service worker → popup / side panel: connection state changed */
export interface ConnectionStateMsg {
  type: 'connection-state';
  state: ConnectionState;
}

/** Popup / side panel → service worker: request current status */
export interface StatusRequestMsg {
  type: 'status-request';
}

/** Service worker → popup / side panel: current status */
export interface StatusResponseMsg {
  type: 'status-response';
  status: ClientStatus;
}

/** Service worker → offscreen: initialize WASM with codebook */
export interface InitWasmMsg {
  type: 'init-wasm';
  sessionId: number[];
  hmacKey: number[];
  codebookData?: number[]; // Serialized codebook (from cache)
}

/** Offscreen → service worker: WASM initialized */
export interface WasmReadyMsg {
  type: 'wasm-ready';
  codebookVersion: number;
  entryCount: number;
}

/** Service worker → offscreen: apply codebook delta */
export interface ApplyDeltaMsg {
  type: 'apply-delta';
  deltaData: number[];
}

/** Offscreen → service worker: delta applied */
export interface DeltaAppliedMsg {
  type: 'delta-applied';
  newVersion: number;
}

/** Service worker → offscreen: build a user action frame */
export interface BuildActionFrameMsg {
  type: 'build-action-frame';
  surfaceId: number;
  actionAddress: number;
  targetAddresses: number[];
}

/** Offscreen → service worker: built frame ready to send */
export interface ActionFrameBuiltMsg {
  type: 'action-frame-built';
  frameData: number[];
}

/** Content script → service worker: page context info */
export interface PageContextMsg {
  type: 'page-context';
  url: string;
  title: string;
  selection?: string;
}

/** Service worker → content script: inject A2UI surface into page */
export interface InjectSurfaceMsg {
  type: 'inject-surface';
  surfaceId: number;
  position: 'sidebar' | 'overlay' | 'inline';
  anchorSelector?: string;
}

// ── Union types ──────────────────────────────────────────────

/** All messages that can be sent through chrome.runtime */
export type ExtensionMessage =
  | DecodeFrameMsg
  | FrameDecodedMsg
  | RenderOperationMsg
  | ConnectMsg
  | DisconnectMsg
  | ConnectionStateMsg
  | StatusRequestMsg
  | StatusResponseMsg
  | InitWasmMsg
  | WasmReadyMsg
  | ApplyDeltaMsg
  | DeltaAppliedMsg
  | BuildActionFrameMsg
  | ActionFrameBuiltMsg
  | PageContextMsg
  | InjectSurfaceMsg;

// ── Data transfer objects ────────────────────────────────────

export type ConnectionState =
  | 'disconnected'
  | 'connecting'
  | 'handshake'
  | 'syncing-codebook'
  | 'connected'
  | 'error';

export interface ClientStatus {
  connectionState: ConnectionState;
  endpoint: string | null;
  sessionId: string | null;
  codebookVersion: number;
  codebookEntries: number;
  framesReceived: number;
  framesSent: number;
  lastError: string | null;
  uptime: number; // seconds since connection
}

export interface DecodedOperationDto {
  opType:
    | 'createSurface'
    | 'updateComponents'
    | 'updateDataModel'
    | 'deleteSurface'
    | 'userAction'
    | 'cognitiveOp'
    | 'raw';
  surfaceId: number;
  details: Record<string, unknown>;
}

// ── Helpers ──────────────────────────────────────────────────

/** Send a message to the service worker and await a response. */
export function sendToBackground<T extends ExtensionMessage>(
  msg: T
): Promise<ExtensionMessage | undefined> {
  return chrome.runtime.sendMessage(msg);
}

/** Send a message to a specific tab's content script. */
export function sendToTab(
  tabId: number,
  msg: ExtensionMessage
): Promise<ExtensionMessage | undefined> {
  return chrome.tabs.sendMessage(tabId, msg);
}

/** Type guard for checking message type. */
export function isMessage<T extends ExtensionMessage['type']>(
  msg: unknown,
  type: T
): msg is Extract<ExtensionMessage, { type: T }> {
  return (
    typeof msg === 'object' &&
    msg !== null &&
    'type' in msg &&
    (msg as { type: string }).type === type
  );
}
