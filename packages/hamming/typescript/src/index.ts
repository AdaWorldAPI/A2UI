/**
 * A2UI Hamming Protocol — TypeScript wrapper for WASM client.
 *
 * This thin wrapper integrates the Rust WASM Hamming client with
 * the Lit/Angular renderer. It handles WASM initialization, frame
 * reception over WebSocket/gRPC-Web, and feeds decoded operations
 * to the A2UI renderer pipeline.
 *
 * @example
 * ```typescript
 * import { HammingTransport } from '@a2ui/hamming';
 *
 * const transport = await HammingTransport.connect('wss://agent.example.com/hamming');
 * transport.onOperation((op) => {
 *   if (op.op_type === 'createSurface') {
 *     renderer.createSurface(op.surface_id, op.details);
 *   }
 * });
 * ```
 */

// Types matching the Rust WASM serialized output
export interface DecodedOperation {
  op_type:
    | 'createSurface'
    | 'updateComponents'
    | 'updateDataModel'
    | 'deleteSurface'
    | 'userAction'
    | 'cognitiveOp'
    | 'raw';
  surface_id: number;
  details: Record<string, unknown>;
}

export interface CodebookStats {
  version: number;
  entry_count: number;
  zones: ZoneStats[];
}

export interface ZoneStats {
  name: string;
  capacity: number;
  used: number;
}

// Well-known codebook addresses (mirrors Rust zones module)
export const Addresses = {
  // Protocol
  HANDSHAKE_INIT: 0x000,
  HEARTBEAT: 0x002,
  CODEBOOK_SYNC: 0x003,

  // Messages
  CREATE_SURFACE: 0x020,
  UPDATE_COMPONENTS: 0x021,
  UPDATE_DATA_MODEL: 0x022,
  DELETE_SURFACE: 0x023,

  // Components
  TEXT: 0x080,
  BUTTON: 0x081,
  CARD: 0x082,
  ROW: 0x083,
  COLUMN: 0x084,
  IMAGE: 0x085,
  TEXT_FIELD: 0x086,
  SLIDER: 0x087,
  CHECKBOX: 0x088,
  MODAL: 0x089,
  TABS: 0x08a,
  LIST: 0x08b,

  // Styles
  THEME_LIGHT: 0x400,
  THEME_DARK: 0x401,

  // Actions
  CLICK: 0x800,
  TEXT_INPUT: 0x801,
  SCROLL: 0x802,
  SUBMIT: 0x805,
} as const;

type OperationHandler = (op: DecodedOperation) => void;
type ErrorHandler = (error: Error) => void;

/**
 * Hamming transport client for the A2UI renderer.
 *
 * Manages WebSocket connection, WASM Hamming decoder,
 * and feeds decoded operations to registered handlers.
 */
export class HammingTransport {
  private wasmClient: any; // WasmHammingClient from Rust WASM
  private ws: WebSocket | null = null;
  private operationHandlers: OperationHandler[] = [];
  private errorHandlers: ErrorHandler[] = [];
  private sessionId: Uint8Array;
  private hmacKey: Uint8Array;
  private sequence: number = 0;
  private connected: boolean = false;

  private constructor(
    wasmClient: any,
    sessionId: Uint8Array,
    hmacKey: Uint8Array
  ) {
    this.wasmClient = wasmClient;
    this.sessionId = sessionId;
    this.hmacKey = hmacKey;
  }

  /**
   * Connect to a Hamming transport endpoint.
   *
   * The endpoint should be a WebSocket URL that bridges to the
   * gRPC Hamming server (or a direct gRPC-Web endpoint).
   */
  static async connect(
    endpoint: string,
    options: ConnectOptions = {}
  ): Promise<HammingTransport> {
    // Import and initialize WASM module
    // In production this would be: const wasm = await import('a2ui-hamming-wasm');
    // For now we provide a stub that gets replaced when WASM is built
    const wasmModule = options.wasmModule;
    if (!wasmModule) {
      throw new Error(
        'WASM module not provided. Build with: wasm-pack build --target web'
      );
    }

    // Session ID and HMAC key from handshake or options
    const sessionId =
      options.sessionId || crypto.getRandomValues(new Uint8Array(8));
    const hmacKey =
      options.hmacKey || crypto.getRandomValues(new Uint8Array(32));

    const client = new wasmModule.WasmHammingClient(sessionId, hmacKey);
    const transport = new HammingTransport(client, sessionId, hmacKey);

    // Connect WebSocket
    if (endpoint.startsWith('ws://') || endpoint.startsWith('wss://')) {
      await transport.connectWebSocket(endpoint);
    }

    return transport;
  }

  /**
   * Create a transport with a pre-initialized WASM client (for testing).
   */
  static fromWasmClient(
    wasmClient: any,
    sessionId: Uint8Array,
    hmacKey: Uint8Array
  ): HammingTransport {
    return new HammingTransport(wasmClient, sessionId, hmacKey);
  }

  /**
   * Register a handler for decoded operations.
   */
  onOperation(handler: OperationHandler): () => void {
    this.operationHandlers.push(handler);
    return () => {
      this.operationHandlers = this.operationHandlers.filter(
        (h) => h !== handler
      );
    };
  }

  /**
   * Register an error handler.
   */
  onError(handler: ErrorHandler): () => void {
    this.errorHandlers.push(handler);
    return () => {
      this.errorHandlers = this.errorHandlers.filter((h) => h !== handler);
    };
  }

  /**
   * Send a user action frame to the server.
   *
   * @param surfaceId - Target surface
   * @param actionAddress - Codebook address for the action type (e.g., Addresses.CLICK)
   * @param targetAddress - Codebook address for the target component
   */
  sendAction(
    surfaceId: number,
    actionAddress: number,
    targetAddress: number
  ): void {
    if (!this.ws || !this.connected) {
      throw new Error('Not connected');
    }

    this.sequence++;
    const frame = this.wasmClient.buildXorFrame(
      surfaceId,
      this.sequence,
      actionAddress,
      new Uint16Array([targetAddress]),
      this.hmacKey
    );

    this.ws.send(frame);
  }

  /**
   * Send a codebook reference frame to the server.
   */
  sendRefs(surfaceId: number, addresses: number[]): void {
    if (!this.ws || !this.connected) {
      throw new Error('Not connected');
    }

    this.sequence++;
    const frame = this.wasmClient.buildRefFrame(
      surfaceId,
      this.sequence,
      new Uint16Array(addresses),
      this.hmacKey
    );

    this.ws.send(frame);
  }

  /**
   * Look up a codebook entry by address.
   */
  lookupAddress(address: number): string | undefined {
    return this.wasmClient.lookupAddress(address) ?? undefined;
  }

  /**
   * Get codebook statistics.
   */
  codebookStats(): CodebookStats {
    return this.wasmClient.codebookStats();
  }

  /**
   * Get the local codebook version.
   */
  codebookVersion(): number {
    return this.wasmClient.codebookVersion();
  }

  /**
   * Disconnect from the server.
   */
  disconnect(): void {
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    this.connected = false;
  }

  // ── Private ──────────────────────────────────────────

  private async connectWebSocket(url: string): Promise<void> {
    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(url);
      this.ws.binaryType = 'arraybuffer';

      this.ws.onopen = () => {
        this.connected = true;
        resolve();
      };

      this.ws.onerror = (event) => {
        const error = new Error('WebSocket connection failed');
        this.errorHandlers.forEach((h) => h(error));
        reject(error);
      };

      this.ws.onmessage = (event) => {
        this.handleMessage(event.data);
      };

      this.ws.onclose = () => {
        this.connected = false;
      };
    });
  }

  private handleMessage(data: ArrayBuffer): void {
    try {
      const bytes = new Uint8Array(data);
      const operation = this.wasmClient.decodeFrame(bytes);
      this.operationHandlers.forEach((h) => h(operation));
    } catch (error) {
      this.errorHandlers.forEach((h) =>
        h(error instanceof Error ? error : new Error(String(error)))
      );
    }
  }
}

export interface ConnectOptions {
  /** Pre-loaded WASM module */
  wasmModule?: any;
  /** Session ID (8 bytes). Auto-generated if not provided. */
  sessionId?: Uint8Array;
  /** HMAC key for frame authentication. Auto-generated if not provided. */
  hmacKey?: Uint8Array;
}
