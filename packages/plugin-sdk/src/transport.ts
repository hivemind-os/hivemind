/**
 * JSON-RPC transport layer for plugin ↔ host communication.
 *
 * Plugins communicate with the Hivemind host via JSON-RPC 2.0 over stdio.
 * This module handles message framing, request/response matching, and
 * dispatching incoming method calls.
 */

import type {
  JsonRpcRequest,
  JsonRpcResponse,
  JsonRpcNotification,
  JsonRpcError,
} from "./types.js";

// ─── Transport ──────────────────────────────────────────────────────────────

export type MethodHandler = (params: unknown) => Promise<unknown>;

export class JsonRpcTransport {
  private nextId = 1;
  private pending = new Map<
    string | number,
    {
      resolve: (value: unknown) => void;
      reject: (error: Error) => void;
    }
  >();
  private handlers = new Map<string, MethodHandler>();
  private buffer = "";
  private input: NodeJS.ReadableStream;
  private output: NodeJS.WritableStream;

  constructor(
    input: NodeJS.ReadableStream = process.stdin,
    output: NodeJS.WritableStream = process.stdout,
  ) {
    this.input = input;
    this.output = output;
  }

  /** Start listening for incoming messages. */
  start(): void {
    this.input.setEncoding?.("utf8" as any);
    this.input.on("data", (chunk: Buffer | string) => {
      this.buffer += chunk.toString();
      this.processBuffer();
    });
  }

  /** Register a handler for incoming method calls from the host. */
  onMethod(method: string, handler: MethodHandler): void {
    this.handlers.set(method, handler);
  }

  /** Send a request to the host and wait for a response. */
  async request(method: string, params?: unknown): Promise<unknown> {
    const id = this.nextId++;
    const msg: JsonRpcRequest = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };

    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.send(msg);
    });
  }

  /** Send a notification to the host (no response expected). */
  notify(method: string, params?: unknown): void {
    const msg: JsonRpcNotification = {
      jsonrpc: "2.0",
      method,
      params,
    };
    this.send(msg);
  }

  private send(msg: JsonRpcRequest | JsonRpcResponse | JsonRpcNotification): void {
    const json = JSON.stringify(msg);
    const header = `Content-Length: ${Buffer.byteLength(json)}\r\n\r\n`;
    this.output.write(header + json);
  }

  private processBuffer(): void {
    while (true) {
      // Parse Content-Length header
      const headerEnd = this.buffer.indexOf("\r\n\r\n");
      if (headerEnd === -1) break;

      const header = this.buffer.substring(0, headerEnd);
      const match = header.match(/Content-Length:\s*(\d+)/i);
      if (!match) {
        // Skip malformed header
        this.buffer = this.buffer.substring(headerEnd + 4);
        continue;
      }

      const contentLength = parseInt(match[1], 10);
      const bodyStart = headerEnd + 4;

      if (this.buffer.length < bodyStart + contentLength) {
        break; // Wait for more data
      }

      const body = this.buffer.substring(bodyStart, bodyStart + contentLength);
      this.buffer = this.buffer.substring(bodyStart + contentLength);

      try {
        const msg = JSON.parse(body);
        this.handleMessage(msg);
      } catch {
        // Skip malformed JSON
      }
    }
  }

  private handleMessage(msg: any): void {
    if ("id" in msg && ("result" in msg || "error" in msg)) {
      // Response to our request
      this.handleResponse(msg as JsonRpcResponse);
    } else if ("method" in msg) {
      if ("id" in msg) {
        // Request from host
        this.handleRequest(msg as JsonRpcRequest);
      } else {
        // Notification from host
        this.handleNotification(msg as JsonRpcNotification);
      }
    }
  }

  private handleResponse(msg: JsonRpcResponse): void {
    const pending = this.pending.get(msg.id);
    if (!pending) return;

    this.pending.delete(msg.id);

    if (msg.error) {
      pending.reject(
        new JsonRpcTransportError(
          msg.error.message,
          msg.error.code,
          msg.error.data,
        ),
      );
    } else {
      pending.resolve(msg.result);
    }
  }

  private async handleRequest(msg: JsonRpcRequest): Promise<void> {
    const handler = this.handlers.get(msg.method);

    if (!handler) {
      this.send({
        jsonrpc: "2.0",
        id: msg.id!,
        error: {
          code: -32601,
          message: `Method not found: ${msg.method}`,
        },
      });
      return;
    }

    try {
      const result = await handler(msg.params);
      this.send({
        jsonrpc: "2.0",
        id: msg.id!,
        result: result ?? null,
      });
    } catch (err) {
      this.send({
        jsonrpc: "2.0",
        id: msg.id!,
        error: {
          code: -32000,
          message: err instanceof Error ? err.message : String(err),
          data:
            err instanceof Error ? { stack: err.stack } : undefined,
        },
      });
    }
  }

  private handleNotification(msg: JsonRpcNotification): void {
    const handler = this.handlers.get(msg.method);
    if (handler) {
      handler(msg.params).catch(() => {
        // Notifications are fire-and-forget
      });
    }
  }
}

// ─── Error ──────────────────────────────────────────────────────────────────

export class JsonRpcTransportError extends Error {
  code: number;
  data?: unknown;

  constructor(message: string, code: number, data?: unknown) {
    super(message);
    this.name = "JsonRpcTransportError";
    this.code = code;
    this.data = data;
  }
}
