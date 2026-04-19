/**
 * JSON-RPC transport layer for plugin ↔ host communication.
 *
 * Plugins communicate with the Hivemind host via JSON-RPC 2.0 over stdio.
 * This module handles message framing, request/response matching, and
 * dispatching incoming method calls.
 */
export type MethodHandler = (params: unknown) => Promise<unknown>;
export declare class JsonRpcTransport {
    private nextId;
    private pending;
    private handlers;
    private buffer;
    private input;
    private output;
    constructor(input?: NodeJS.ReadableStream, output?: NodeJS.WritableStream);
    /** Start listening for incoming messages. */
    start(): void;
    /** Register a handler for incoming method calls from the host. */
    onMethod(method: string, handler: MethodHandler): void;
    /** Send a request to the host and wait for a response. */
    request(method: string, params?: unknown): Promise<unknown>;
    /** Send a notification to the host (no response expected). */
    notify(method: string, params?: unknown): void;
    private send;
    private processBuffer;
    private handleMessage;
    private handleResponse;
    private handleRequest;
    private handleNotification;
}
export declare class JsonRpcTransportError extends Error {
    code: number;
    data?: unknown;
    constructor(message: string, code: number, data?: unknown);
}
//# sourceMappingURL=transport.d.ts.map