/**
 * JSON-RPC transport layer for plugin ↔ host communication.
 *
 * Plugins communicate with the Hivemind host via JSON-RPC 2.0 over stdio.
 * This module handles message framing, request/response matching, and
 * dispatching incoming method calls.
 */
export class JsonRpcTransport {
    nextId = 1;
    pending = new Map();
    handlers = new Map();
    buffer = "";
    input;
    output;
    constructor(input = process.stdin, output = process.stdout) {
        this.input = input;
        this.output = output;
    }
    /** Start listening for incoming messages. */
    start() {
        this.input.setEncoding?.("utf8");
        this.input.on("data", (chunk) => {
            this.buffer += chunk.toString();
            this.processBuffer();
        });
    }
    /** Register a handler for incoming method calls from the host. */
    onMethod(method, handler) {
        this.handlers.set(method, handler);
    }
    /** Send a request to the host and wait for a response. */
    async request(method, params) {
        const id = this.nextId++;
        const msg = {
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
    notify(method, params) {
        const msg = {
            jsonrpc: "2.0",
            method,
            params,
        };
        this.send(msg);
    }
    send(msg) {
        const json = JSON.stringify(msg);
        const header = `Content-Length: ${Buffer.byteLength(json)}\r\n\r\n`;
        this.output.write(header + json);
    }
    processBuffer() {
        while (true) {
            // Parse Content-Length header
            const headerEnd = this.buffer.indexOf("\r\n\r\n");
            if (headerEnd === -1)
                break;
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
            }
            catch {
                // Skip malformed JSON
            }
        }
    }
    handleMessage(msg) {
        if ("id" in msg && ("result" in msg || "error" in msg)) {
            // Response to our request
            this.handleResponse(msg);
        }
        else if ("method" in msg) {
            if ("id" in msg) {
                // Request from host
                this.handleRequest(msg);
            }
            else {
                // Notification from host
                this.handleNotification(msg);
            }
        }
    }
    handleResponse(msg) {
        const pending = this.pending.get(msg.id);
        if (!pending)
            return;
        this.pending.delete(msg.id);
        if (msg.error) {
            pending.reject(new JsonRpcTransportError(msg.error.message, msg.error.code, msg.error.data));
        }
        else {
            pending.resolve(msg.result);
        }
    }
    async handleRequest(msg) {
        const handler = this.handlers.get(msg.method);
        if (!handler) {
            this.send({
                jsonrpc: "2.0",
                id: msg.id,
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
                id: msg.id,
                result: result ?? null,
            });
        }
        catch (err) {
            this.send({
                jsonrpc: "2.0",
                id: msg.id,
                error: {
                    code: -32000,
                    message: err instanceof Error ? err.message : String(err),
                    data: err instanceof Error ? { stack: err.stack } : undefined,
                },
            });
        }
    }
    handleNotification(msg) {
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
    code;
    data;
    constructor(message, code, data) {
        super(message);
        this.name = "JsonRpcTransportError";
        this.code = code;
        this.data = data;
    }
}
//# sourceMappingURL=transport.js.map