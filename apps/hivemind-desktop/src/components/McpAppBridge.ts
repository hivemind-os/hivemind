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

/** Tool definition declared by an MCP App via appCapabilities.tools */
export interface AppToolDefinition {
  name: string;
  description?: string;
  inputSchema?: Record<string, unknown>;
}

// ── Bridge configuration ────────────────────────────────────────────

export interface McpAppBridgeConfig {
  iframe: HTMLIFrameElement;
  /** Unique identifier for this app instance */
  appInstanceId: string;
  serverId: string;
  toolName: string;
  toolInput?: string;
  toolOutput?: string;
  toolIsError?: boolean;
  /** Raw tool result JSON from the MCP server (full CallToolResult shape) */
  toolResultRaw?: unknown;
  /** Tool input schema (JSON Schema) for hostContext.toolInfo */
  toolInputSchema?: Record<string, unknown>;
  /** Tool description for hostContext.toolInfo */
  toolDescription?: string;
  sessionId: string;
  daemonUrl: string;
  theme: 'light' | 'dark';
  /** Current display mode (spec: inline | fullscreen | pip) */
  displayMode?: 'inline' | 'fullscreen' | 'pip';
  /** Available display modes */
  availableDisplayModes?: ('inline' | 'fullscreen' | 'pip')[];
  /** Container dimensions (width × height in px) */
  containerWidth?: number;
  containerHeight?: number;
  /** Tool visibility list (e.g. ["model","app"]) */
  toolVisibility?: string[];
  onSizeChanged?: (width: number, height: number) => void;
  onMessage?: (content: string) => void;
  onOpenLink?: (url: string) => void;
  onModelContextUpdate?: (context: Record<string, unknown>) => void;
  onPopout?: () => void;
  /** Called when the app completes the ui/initialize handshake */
  onInitialized?: () => void;
  /** Called when the app registers or updates its tool list */
  onAppToolsChanged?: (tools: AppToolDefinition[]) => void;
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
  /** Tools declared by the app via appCapabilities.tools */
  private appTools: AppToolDefinition[] = [];

  constructor(config: McpAppBridgeConfig) {
    this.config = config;
    this.iframe = config.iframe;
    this.messageHandler = this.handleMessage.bind(this);
    window.addEventListener('message', this.messageHandler);
  }

  /** Unique identifier for this app instance. */
  get appInstanceId(): string {
    return this.config.appInstanceId;
  }

  /** Clean up event listeners. Call before removing the iframe. */
  destroy(): void {
    // Send teardown as a request (spec: has id, expects response)
    this.sendRequest('ui/resource-teardown', {}).catch(() => {});
    if (this.messageHandler) {
      window.removeEventListener('message', this.messageHandler);
      this.messageHandler = null;
    }
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

  /** Send partial/streaming tool input before the final tool-input. */
  sendToolInputPartial(partialInput: string): void {
    this.sendNotification('ui/notifications/tool-input-partial', {
      arguments: tryParseJson(partialInput) ?? {},
    });
  }

  /** Send tool result to the app (full CallToolResult shape). */
  sendToolResult(output: string, isError?: boolean, rawResult?: unknown): void {
    // If we have the raw MCP result, forward it as-is (preserves structuredContent, _meta, multi-block content)
    if (rawResult && typeof rawResult === 'object') {
      this.sendNotification('ui/notifications/tool-result', rawResult);
      return;
    }
    // Fallback: reconstruct from plain text
    const parsed = tryParseJson(output);
    const content = parsed != null
      ? [{ type: 'text', text: typeof parsed === 'string' ? parsed : JSON.stringify(parsed) }]
      : [{ type: 'text', text: output ?? '' }];
    this.sendNotification('ui/notifications/tool-result', {
      content,
      isError: isError ?? false,
    });
  }

  /** Notify the app of tool cancellation. */
  sendToolCancelled(reason?: string): void {
    this.sendNotification('ui/notifications/tool-cancelled', {
      ...(reason ? { reason } : {}),
    });
  }

  /** Notify the app of a theme or context change. Params are the partial HostContext directly. */
  sendHostContextChanged(context: Record<string, unknown>): void {
    this.sendNotification('ui/notifications/host-context-changed', context);
  }

  /** Update container dimensions (e.g. on resize or popout). */
  updateContainerDimensions(width: number, height: number): void {
    this.config.containerWidth = width;
    this.config.containerHeight = height;
    if (this.initialized) {
      this.sendHostContextChanged({ containerDimensions: { width, height } });
    }
  }

  /** Update display mode (e.g. inline → fullscreen). */
  updateDisplayMode(mode: 'inline' | 'fullscreen' | 'pip'): void {
    this.config.displayMode = mode;
    if (this.initialized) {
      this.sendHostContextChanged({ displayMode: mode });
    }
  }

  // ── App-registered tools ─────────────────────────────────────

  /** Get the current list of app-registered tools. */
  getAppTools(): AppToolDefinition[] {
    return [...this.appTools];
  }

  /**
   * Call a tool registered by the app. Sends tools/call as a JSON-RPC
   * request TO the iframe and returns the result.
   */
  async callAppTool(name: string, args?: Record<string, unknown>): Promise<unknown> {
    return this.sendRequest('tools/call', { name, arguments: args ?? {} });
  }

  /** Re-fetch app tools via tools/list and notify listeners. */
  private async refreshAppTools(): Promise<void> {
    try {
      const result = await this.sendRequest('tools/list', {}) as { tools?: AppToolDefinition[] };
      this.appTools = result?.tools ?? [];
      this.config.onAppToolsChanged?.(this.appTools);
    } catch (err) {
      console.warn('[McpAppBridge] Failed to refresh app tools:', err);
    }
  }

  // ── Inbound: App → Host ───────────────────────────────────────

  private handleMessage(event: MessageEvent): void {
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
      this.sendErrorResponse(msg.id, err.code ?? -32603, err.message ?? 'Internal error');
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

      case 'resources/list':
        return this.handleResourcesList();

      case 'ui/open-link':
        return this.handleOpenLink(params as { url: string });

      case 'ui/download-file':
        return this.handleDownloadFile(params as { contents: Array<Record<string, unknown>> });

      case 'ui/message':
        return this.handleUiMessage(params as { role: string; content: Array<{ type: string; text?: string }> });

      case 'ui/update-model-context':
        return this.handleUpdateModelContext(params as Record<string, unknown>);

      case 'sampling/createMessage':
        return this.handleSamplingCreateMessage(params as Record<string, unknown>);

      case 'ui/request-display-mode':
        return { mode: this.config.displayMode ?? 'inline' };

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
        this.config.onInitialized?.();
        if (this.config.toolInput) {
          this.sendToolInput(this.config.toolInput);
        }
        if (this.config.toolOutput != null) {
          this.sendToolResult(this.config.toolOutput, this.config.toolIsError, this.config.toolResultRaw);
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
        break;

      case 'notifications/message': {
        const logParams = msg.params as { level?: string; data?: unknown } | undefined;
        console.log(`[MCP App:${this.config.toolName}]`, logParams?.level ?? 'info', logParams?.data ?? '');
        break;
      }

      case 'notifications/tools/list_changed':
        // App's tool list changed — re-fetch via tools/list
        this.refreshAppTools();
        break;

      default:
        console.debug('[McpAppBridge] Unknown notification:', msg.method);
    }
  }

  // ── Request handlers ──────────────────────────────────────────

  private handleInitialize(params: unknown): Record<string, unknown> {
    const p = params as {
      appInfo?: unknown;
      appCapabilities?: { tools?: AppToolDefinition[] };
      protocolVersion?: string;
    } | undefined;

    // Store app-registered tools if declared
    if (p?.appCapabilities?.tools && Array.isArray(p.appCapabilities.tools)) {
      this.appTools = p.appCapabilities.tools;
      this.config.onAppToolsChanged?.(this.appTools);
    }

    return {
      protocolVersion: '2026-01-26',
      hostInfo: {
        name: 'hivemind-desktop',
        version: '1.0.0',
      },
      hostCapabilities: {
        openLinks: {},
        downloadFile: {},
        serverTools: {},
        serverResources: {},
        logging: {},
        message: { text: {} },
        updateModelContext: {},
        sampling: {},
        sandbox: {},
      },
      hostContext: this.buildHostContext(),
    };
  }

  private async handleToolsCall(params: { name: string; arguments?: Record<string, unknown> }): Promise<unknown> {
    // Visibility check: reject if the target tool doesn't have "app" visibility
    // (Default visibility is ["model","app"] so most tools are allowed)
    const toolVisibility = this.config.toolVisibility;
    if (toolVisibility && !toolVisibility.includes('app')) {
      throw { code: -32600, message: `Tool "${params.name}" is not accessible from apps` };
    }

    const resp = await authFetch(
      `${this.config.daemonUrl}/api/v1/mcp/servers/${encodeURIComponent(this.config.serverId)}/call-tool`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: params.name, arguments: params.arguments ?? {} }),
      },
    );
    if (!resp.ok) throw new Error(`Tool call failed: ${resp.status}`);
    const data = await resp.json() as { content: string; is_error: boolean; raw?: unknown };
    // Return the raw CallToolResult if available (preserves structuredContent for apps),
    // otherwise reconstruct a minimal CallToolResult from the flattened fields.
    if (data.raw && typeof data.raw === 'object') {
      return data.raw;
    }
    return {
      content: [{ type: 'text', text: data.content ?? '' }],
      isError: data.is_error ?? false,
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
    return await resp.json();
  }

  private async handleResourcesList(): Promise<unknown> {
    const resp = await authFetch(
      `${this.config.daemonUrl}/api/v1/mcp/servers/${encodeURIComponent(this.config.serverId)}/resources`,
    );
    if (!resp.ok) throw new Error(`Resource list failed: ${resp.status}`);
    const resources = await resp.json();
    // Daemon returns an array; MCP spec expects { resources: [...] }
    return { resources: Array.isArray(resources) ? resources : [] };
  }

  private async handleDownloadFile(params: { contents: Array<Record<string, unknown>> }): Promise<Record<string, unknown>> {
    try {
      for (const item of params.contents ?? []) {
        // EmbeddedResource: has blob (base64) or text content inline
        if (item.type === 'resource' && item.resource && typeof item.resource === 'object') {
          const res = item.resource as Record<string, unknown>;
          const uri = (res.uri as string) ?? 'download';
          const filename = uri.split('/').pop() ?? 'download';
          const mimeType = (res.mimeType as string) ?? 'application/octet-stream';

          let blob: Blob;
          if (typeof res.blob === 'string') {
            // Base64-encoded binary data
            const binary = atob(res.blob as string);
            const bytes = new Uint8Array(binary.length);
            for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
            blob = new Blob([bytes], { type: mimeType });
          } else if (typeof res.text === 'string') {
            blob = new Blob([res.text as string], { type: mimeType });
          } else {
            continue;
          }

          // Trigger download via temporary anchor in the main window
          const url = URL.createObjectURL(blob);
          const a = document.createElement('a');
          a.href = url;
          a.download = filename;
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
          URL.revokeObjectURL(url);
        }
        // ResourceLink: has a uri to fetch
        else if (item.type === 'resource_link' && typeof item.uri === 'string') {
          const uri = item.uri as string;
          const filename = uri.split('/').pop() ?? 'download';
          // Fetch via MCP server resource read
          try {
            const resp = await authFetch(
              `${this.config.daemonUrl}/api/v1/mcp/servers/${encodeURIComponent(this.config.serverId)}/read-resource`,
              {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ uri }),
              },
            );
            if (resp.ok) {
              const data = await resp.json() as { contents?: Array<{ text?: string; blob?: string; mimeType?: string }> };
              const content = data.contents?.[0];
              if (content) {
                const mimeType = content.mimeType ?? 'application/octet-stream';
                let blob: Blob;
                if (content.blob) {
                  const binary = atob(content.blob);
                  const bytes = new Uint8Array(binary.length);
                  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
                  blob = new Blob([bytes], { type: mimeType });
                } else if (content.text) {
                  blob = new Blob([content.text], { type: mimeType });
                } else {
                  continue;
                }
                const url = URL.createObjectURL(blob);
                const a = document.createElement('a');
                a.href = url;
                a.download = filename;
                document.body.appendChild(a);
                a.click();
                document.body.removeChild(a);
                URL.revokeObjectURL(url);
              }
            }
          } catch {
            // Skip failed resource links
          }
        }
      }
      return {};
    } catch (e: any) {
      return { isError: true };
    }
  }

  private async handleOpenLink(params: { url: string }): Promise<unknown> {
    const url = params.url;
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
      const text = params.content
        ?.filter((b) => b.type === 'text' && b.text)
        .map((b) => b.text)
        .join('\n') ?? '';
      this.config.onMessage(text);
    }
    return {};
  }

  private handleUpdateModelContext(params: Record<string, unknown>): Record<string, unknown> {
    if (this.config.onModelContextUpdate) {
      this.config.onModelContextUpdate(params);
    }
    return {};
  }

  private async handleSamplingCreateMessage(params: Record<string, unknown>): Promise<Record<string, unknown>> {
    // Proxy sampling request to the daemon's completion API
    const resp = await authFetch(
      `${this.config.daemonUrl}/api/v1/mcp/sampling/create-message`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(params),
      },
    );
    if (!resp.ok) {
      const errorText = await resp.text().catch(() => '');
      throw { code: -32603, message: `Sampling failed: ${resp.status} ${errorText}` };
    }
    return await resp.json();
  }

  // ── Helpers ───────────────────────────────────────────────────

  private buildHostContext(): Record<string, unknown> {
    const ctx: Record<string, unknown> = {
      theme: this.config.theme,
      platform: 'desktop',
      locale: navigator.language,
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      userAgent: 'hivemind-desktop',
      displayMode: this.config.displayMode ?? 'inline',
      availableDisplayModes: this.config.availableDisplayModes ?? ['inline'],
    };

    // toolInfo: apps need to know which tool was called and its schema
    // Spec: toolInfo.tool is a standard MCP Tool object { name, description, inputSchema }
    if (this.config.toolName) {
      ctx.toolInfo = {
        tool: {
          name: this.config.toolName,
          ...(this.config.toolDescription ? { description: this.config.toolDescription } : {}),
          ...(this.config.toolInputSchema ? { inputSchema: this.config.toolInputSchema } : { inputSchema: { type: 'object' } }),
        },
      };
    }

    // Container dimensions
    if (this.config.containerWidth != null && this.config.containerHeight != null) {
      ctx.containerDimensions = {
        width: this.config.containerWidth,
        height: this.config.containerHeight,
      };
    }

    // Theme CSS variables (map from Hivemind's design tokens)
    ctx.styles = {
      variables: getThemeVariables(),
    };

    return ctx;
  }

  private sendNotification(method: string, params: unknown): void {
    this.postToApp({ jsonrpc: '2.0', method, params });
  }

  /** Send a JSON-RPC request and return a promise for the response. */
  private sendRequest(method: string, params: unknown): Promise<unknown> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
      this.postToApp({ jsonrpc: '2.0', id, method, params });
      // Timeout after 5s
      setTimeout(() => {
        if (this.pendingRequests.has(id)) {
          this.pendingRequests.delete(id);
          reject(new Error(`Request ${method} timed out`));
        }
      }, 5000);
    });
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

/** Read theme CSS variables from the document for the MCP Apps styles.variables spec.
 *  Keys must match McpUiStyleVariableKey from the spec (e.g. --color-background-primary). */
function getThemeVariables(): Record<string, string> {
  try {
    const styles = getComputedStyle(document.documentElement);
    const vars: Record<string, string> = {};
    // Map standardized MCP Apps variable names → Hivemind CSS custom properties
    const mapping: Record<string, string> = {
      '--color-background-primary': '--background',
      '--color-background-secondary': '--secondary',
      '--color-background-tertiary': '--muted',
      '--color-text-primary': '--foreground',
      '--color-text-secondary': '--muted-foreground',
      '--color-text-inverse': '--primary-foreground',
      '--color-text-danger': '--destructive',
      '--color-border-primary': '--border',
      '--color-ring-primary': '--ring',
      '--border-radius-sm': '--radius',
    };
    for (const [specVar, cssVar] of Object.entries(mapping)) {
      const val = styles.getPropertyValue(cssVar).trim();
      if (val) vars[specVar] = val;
    }
    return vars;
  } catch {
    return {};
  }
}

function tryParseJson(s: string | undefined): unknown | undefined {
  if (!s) return undefined;
  try { return JSON.parse(s); } catch { return undefined; }
}
