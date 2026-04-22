/**
 * McpAppBridge — Host-side implementation of the MCP Apps postMessage protocol.
 *
 * Manages bidirectional JSON-RPC communication between the sandboxed iframe
 * (MCP App) and the Hivemind host.
 *
 * Spec: https://modelcontextprotocol.io/extensions/apps/specification
 */

import { authFetch } from '~/lib/authFetch';
import { openExternal } from '../utils';

// ── JSON-RPC types ──────────────────────────────────────────────────

interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: string | number;
  method: string;
  params?: unknown;
}

interface JsonRpcNotification {
  jsonrpc: '2.0';
  method: string;
  params?: unknown;
}

interface JsonRpcResponse {
  jsonrpc: '2.0';
  id: string | number;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
}

type JsonRpcMessage = JsonRpcRequest | JsonRpcNotification | JsonRpcResponse;

// ── Bridge configuration ────────────────────────────────────────────

export interface McpAppBridgeConfig {
  iframe: HTMLIFrameElement;
  serverId: string;
  toolName: string;
  toolInput?: string;
  toolOutput?: string;
  toolIsError?: boolean;
  sessionId: string;
  daemonUrl: string;
  theme: 'light' | 'dark';
  onSizeChanged?: (width: number, height: number) => void;
  onMessage?: (content: string) => void;
  onOpenLink?: (url: string) => void;
}

// ── Bridge implementation ───────────────────────────────────────────

export class McpAppBridge {
  private iframe: HTMLIFrameElement;
  private config: McpAppBridgeConfig;
  private pendingRequests = new Map<string | number, {
    resolve: (value: unknown) => void;
    reject: (reason: unknown) => void;
  }>();
  private initialized = false;
  private nextId = 1;
  private messageHandler: ((event: MessageEvent) => void) | null = null;

  constructor(config: McpAppBridgeConfig) {
    this.config = config;
    this.iframe = config.iframe;
    this.messageHandler = this.handleMessage.bind(this);
    window.addEventListener('message', this.messageHandler);
  }

  /** Clean up event listeners. Call before removing the iframe. */
  destroy(): void {
    // Send teardown notification before disconnecting
    this.sendNotification('ui/resource-teardown', {});
    if (this.messageHandler) {
      window.removeEventListener('message', this.messageHandler);
      this.messageHandler = null;
    }
    // Reject pending requests
    for (const [, pending] of this.pendingRequests) {
      pending.reject(new Error('Bridge destroyed'));
    }
    this.pendingRequests.clear();
  }

  // ── Outbound: Host → App ──────────────────────────────────────

  /** Send tool input after initialization completes. */
  sendToolInput(input: string): void {
    this.sendNotification('ui/notifications/tool-input', {
      arguments: tryParseJson(input) ?? {},
    });
  }

  /** Send tool result to the app. */
  sendToolResult(output: string, isError?: boolean): void {
    // Per spec, tool-result params follow CallToolResult shape
    const parsed = tryParseJson(output);
    const content = parsed != null
      ? [{ type: 'text', text: typeof parsed === 'string' ? parsed : JSON.stringify(parsed) }]
      : [{ type: 'text', text: output ?? '' }];
    this.sendNotification('ui/notifications/tool-result', {
      content,
      isError: isError ?? false,
    });
  }

  /** Notify the app of a theme or context change. */
  sendHostContextChanged(context: Record<string, unknown>): void {
    this.sendNotification('ui/notifications/host-context-changed', { hostContext: context });
  }

  // ── Inbound: App → Host ───────────────────────────────────────

  private handleMessage(event: MessageEvent): void {
    // Only accept messages from our iframe
    if (event.source !== this.iframe.contentWindow) return;

    const msg = event.data as JsonRpcMessage;
    if (!msg || msg.jsonrpc !== '2.0') return;

    // Response to our outbound request
    if ('id' in msg && !('method' in msg)) {
      const pending = this.pendingRequests.get(msg.id);
      if (pending) {
        this.pendingRequests.delete(msg.id);
        if ('error' in msg && msg.error) {
          pending.reject(msg.error);
        } else {
          pending.resolve(msg.result);
        }
      }
      return;
    }

    // Request or notification from app
    if ('method' in msg) {
      if ('id' in msg && msg.id != null) {
        this.handleRequest(msg as JsonRpcRequest);
      } else {
        this.handleNotification(msg as JsonRpcNotification);
      }
    }
  }

  private async handleRequest(msg: JsonRpcRequest): Promise<void> {
    try {
      const result = await this.dispatchRequest(msg.method, msg.params);
      this.sendResponse(msg.id, result);
    } catch (err: any) {
      this.sendErrorResponse(msg.id, -32603, err.message ?? 'Internal error');
    }
  }

  private async dispatchRequest(method: string, params: unknown): Promise<unknown> {
    switch (method) {
      case 'ui/initialize':
        return this.handleInitialize(params);

      case 'tools/call':
        return this.handleToolsCall(params as { name: string; arguments?: Record<string, unknown> });

      case 'resources/read':
        return this.handleResourcesRead(params as { uri: string });

      case 'ui/open-link':
        return this.handleOpenLink(params as { url: string });

      case 'ui/message':
        return this.handleUiMessage(params as { role: string; content: Array<{ type: string; text?: string }> });

      case 'ui/update-model-context':
        // Acknowledge context updates (we don't use them currently)
        return {};

      case 'ui/request-display-mode':
        // Acknowledge but only inline is supported in v1
        return { mode: 'inline' };

      case 'ping':
        return {};

      default:
        throw { code: -32601, message: `Method not found: ${method}` };
    }
  }

  private handleNotification(msg: JsonRpcNotification): void {
    switch (msg.method) {
      case 'ui/notifications/initialized':
        this.initialized = true;
        // Now send tool input and result
        if (this.config.toolInput) {
          this.sendToolInput(this.config.toolInput);
        }
        if (this.config.toolOutput != null) {
          this.sendToolResult(this.config.toolOutput, this.config.toolIsError);
        }
        break;

      case 'ui/notifications/size-changed': {
        const p = msg.params as { width?: number; height?: number } | undefined;
        if (p && this.config.onSizeChanged) {
          this.config.onSizeChanged(p.width ?? 0, p.height ?? 0);
        }
        break;
      }

      case 'ui/notifications/request-teardown':
        // App requested teardown — ignore for now (inline view stays)
        break;

      case 'notifications/message':
        // Log message from app (logging notification)
        break;

      case 'notifications/tools/list_changed':
        // App's tools changed — not used currently
        break;

      default:
        console.debug('[McpAppBridge] Unknown notification:', msg.method);
    }
  }

  // ── Request handlers ──────────────────────────────────────────

  private handleInitialize(_params: unknown): Record<string, unknown> {
    return {
      protocolVersion: '2026-01-26',
      hostInfo: {
        name: 'hivemind-desktop',
        version: '1.0.0',
      },
      hostCapabilities: {
        openLinks: {},
        serverTools: {},
        serverResources: {},
        logging: {},
        message: { text: {} },
      },
      hostContext: this.buildHostContext(),
    };
  }

  private async handleToolsCall(params: { name: string; arguments?: Record<string, unknown> }): Promise<unknown> {
    const resp = await authFetch(
      `${this.config.daemonUrl}/api/v1/mcp/servers/${encodeURIComponent(this.config.serverId)}/call-tool`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: params.name, arguments: params.arguments ?? {} }),
      },
    );
    if (!resp.ok) throw new Error(`Tool call failed: ${resp.status}`);
    const result = await resp.json() as { content: string; is_error: boolean };
    return {
      content: [{ type: 'text', text: result.content }],
      isError: result.is_error,
    };
  }

  private async handleResourcesRead(params: { uri: string }): Promise<unknown> {
    const resp = await authFetch(
      `${this.config.daemonUrl}/api/v1/mcp/servers/${encodeURIComponent(this.config.serverId)}/read-resource`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ uri: params.uri }),
      },
    );
    if (!resp.ok) throw new Error(`Resource read failed: ${resp.status}`);
    const content = await resp.json() as string;
    return {
      contents: [{ uri: params.uri, text: content }],
    };
  }

  private async handleOpenLink(params: { url: string }): Promise<unknown> {
    const url = params.url;
    // Only allow http/https links
    if (url.startsWith('http://') || url.startsWith('https://')) {
      if (this.config.onOpenLink) {
        this.config.onOpenLink(url);
      } else {
        await openExternal(url);
      }
    }
    return {};
  }

  private handleUiMessage(params: { role: string; content: Array<{ type: string; text?: string }> }): Record<string, unknown> {
    if (this.config.onMessage) {
      // Extract text from content blocks
      const text = params.content
        ?.filter((b) => b.type === 'text' && b.text)
        .map((b) => b.text)
        .join('\n') ?? '';
      this.config.onMessage(text);
    }
    return {};
  }

  // ── Helpers ───────────────────────────────────────────────────

  private buildHostContext(): Record<string, unknown> {
    return {
      theme: this.config.theme,
      platform: 'desktop',
      locale: navigator.language,
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    };
  }

  private sendNotification(method: string, params: unknown): void {
    this.postToApp({ jsonrpc: '2.0', method, params });
  }

  private sendResponse(id: string | number, result: unknown): void {
    this.postToApp({ jsonrpc: '2.0', id, result });
  }

  private sendErrorResponse(id: string | number, code: number, message: string): void {
    this.postToApp({ jsonrpc: '2.0', id, error: { code, message } });
  }

  private postToApp(msg: JsonRpcMessage): void {
    this.iframe.contentWindow?.postMessage(msg, '*');
  }
}

function tryParseJson(s: string | undefined): unknown | undefined {
  if (!s) return undefined;
  try { return JSON.parse(s); } catch { return undefined; }
}
