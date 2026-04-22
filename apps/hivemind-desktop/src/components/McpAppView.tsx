/**
 * McpAppView — Renders an MCP App HTML resource inside a sandboxed iframe.
 *
 * Used inline in the chat message stream when a tool call has an associated
 * `ui://` resource (detected via `ui_meta.resource_uri` on the tool).
 */

import { createSignal, createEffect, onCleanup, Show } from 'solid-js';
import type { McpToolUiMeta } from '../types';
import { McpAppBridge, type McpAppBridgeConfig } from './McpAppBridge';

export interface McpAppViewProps {
  /** HTML content of the MCP App (from resources/read of ui:// resource) */
  html: string;
  /** MCP server ID that owns this tool */
  serverId: string;
  /** Tool name */
  toolName: string;
  /** Tool input JSON */
  toolInput?: string;
  /** Tool output text */
  toolOutput?: string;
  /** Whether tool result was an error */
  toolIsError?: boolean;
  /** Raw tool result JSON (full CallToolResult shape) */
  toolResultRaw?: unknown;
  /** Tool input schema (JSON Schema) */
  toolInputSchema?: Record<string, unknown>;
  /** Session ID */
  sessionId: string;
  /** Daemon API URL */
  daemonUrl: string;
  /** UI metadata from the tool's _meta.ui */
  uiMeta?: McpToolUiMeta;
  /** Current theme */
  theme?: 'light' | 'dark';
  /** Display mode for this view */
  displayMode?: 'inline' | 'popout';
  /** Tool visibility list */
  toolVisibility?: string[];
  /** Callback when app requests to inject a message */
  onMessage?: (content: string) => void;
  /** Callback when app sends model context update */
  onModelContextUpdate?: (context: Record<string, unknown>) => void;
  /** Callback to popout the app into a larger dialog */
  onPopout?: () => void;
}

/** Default restrictive CSP for MCP Apps with no declared CSP. */
function buildCspMeta(uiMeta?: McpToolUiMeta): string {
  if (!uiMeta?.csp) {
    return `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; media-src 'self' data:; font-src 'none'; object-src 'none'; connect-src 'none';">`;
  }
  const csp = uiMeta.csp;
  const parts: string[] = ["default-src 'none'", "object-src 'none'"];
  const connectSrc = csp.connect_domains?.length ? csp.connect_domains.join(' ') : "'none'";
  parts.push(`connect-src ${connectSrc}`);
  parts.push("script-src 'self' 'unsafe-inline'" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : ''));
  parts.push("style-src 'self' 'unsafe-inline'" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : ''));
  parts.push("img-src 'self' data:" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : ''));
  parts.push("media-src 'self' data:" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : ''));
  parts.push("font-src 'self'" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : ''));
  if (csp.frame_domains?.length) {
    parts.push(`frame-src ${csp.frame_domains.join(' ')}`);
  }
  if (csp.base_uri_domains?.length) {
    parts.push(`base-uri ${csp.base_uri_domains.join(' ')}`);
  }
  return `<meta http-equiv="Content-Security-Policy" content="${parts.join('; ')}">`;
}

/** Build sandbox attribute string based on permissions. */
function buildSandbox(_uiMeta?: McpToolUiMeta): string {
  // Always allow scripts; never allow same-origin
  return 'allow-scripts';
}

/** Build the iframe `allow` attribute from permissions metadata. */
function buildAllow(uiMeta?: McpToolUiMeta): string | undefined {
  if (!uiMeta?.permissions) return undefined;
  const perms: string[] = [];
  if (uiMeta.permissions.camera != null) perms.push('camera');
  if (uiMeta.permissions.microphone != null) perms.push('microphone');
  if (uiMeta.permissions.geolocation != null) perms.push('geolocation');
  if (uiMeta.permissions.clipboard_write != null) perms.push('clipboard-write');
  return perms.length > 0 ? perms.join('; ') : undefined;
}

export default function McpAppView(props: McpAppViewProps) {
  const [iframeHeight, setIframeHeight] = createSignal(300);
  let iframeRef: HTMLIFrameElement | undefined;
  let containerRef: HTMLDivElement | undefined;
  let bridge: McpAppBridge | undefined;

  // Inject CSP meta tag into the app HTML
  const preparedHtml = () => {
    const cspTag = buildCspMeta(props.uiMeta);
    const html = props.html;
    if (html.includes('<head>')) {
      return html.replace('<head>', `<head>${cspTag}`);
    } else if (html.includes('<html>')) {
      return html.replace('<html>', `<html><head>${cspTag}</head>`);
    }
    return `<head>${cspTag}</head>${html}`;
  };

  createEffect(() => {
    if (!iframeRef) return;

    // Measure container for containerDimensions
    const containerWidth = containerRef?.clientWidth ?? 600;
    const containerHeight = containerRef?.clientHeight ?? 400;

    bridge = new McpAppBridge({
      iframe: iframeRef,
      serverId: props.serverId,
      toolName: props.toolName,
      toolInput: props.toolInput,
      toolOutput: props.toolOutput,
      toolIsError: props.toolIsError,
      toolResultRaw: props.toolResultRaw,
      toolInputSchema: props.toolInputSchema,
      sessionId: props.sessionId,
      daemonUrl: props.daemonUrl,
      theme: props.theme ?? 'dark',
      displayMode: props.displayMode ?? 'inline',
      availableDisplayModes: ['inline'],
      containerWidth,
      containerHeight,
      toolVisibility: props.toolVisibility,
      onSizeChanged: (_w, h) => {
        if (h > 0 && h < 2000) setIframeHeight(h);
      },
      onMessage: props.onMessage,
      onModelContextUpdate: props.onModelContextUpdate,
    });
  });

  onCleanup(() => {
    bridge?.destroy();
  });

  const border = () => props.uiMeta?.prefers_border !== false;
  const allowAttr = () => buildAllow(props.uiMeta);

  return (
    <div
      ref={containerRef}
      class="mcp-app-container mt-2 overflow-hidden rounded-lg"
      classList={{
        'border border-border': border(),
      }}
    >
      <div class="flex items-center gap-2 bg-muted/30 px-3 py-1.5 text-xs text-muted-foreground">
        <span class="font-medium">⚡ {props.toolName}</span>
        <span class="opacity-60">MCP App</span>
        <div class="flex-1" />
        <Show when={props.onPopout}>
          <button
            class="inline-flex items-center justify-center rounded p-0.5 hover:bg-muted/60 text-muted-foreground hover:text-foreground transition-colors"
            onClick={() => props.onPopout?.()}
            title="Open in larger view"
            aria-label="Pop out MCP App"
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <polyline points="15 3 21 3 21 9" />
              <line x1="10" y1="14" x2="21" y2="3" />
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h4" />
            </svg>
          </button>
        </Show>
      </div>
      <iframe
        ref={iframeRef}
        srcdoc={preparedHtml()}
        sandbox={buildSandbox(props.uiMeta)}
        allow={allowAttr()}
        style={{
          width: '100%',
          height: `${iframeHeight()}px`,
          border: 'none',
          'background-color': 'transparent',
        }}
        title={`MCP App: ${props.toolName}`}
      />
    </div>
  );
}
