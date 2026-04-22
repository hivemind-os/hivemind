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
  /** Session ID */
  sessionId: string;
  /** Daemon API URL */
  daemonUrl: string;
  /** UI metadata from the tool's _meta.ui */
  uiMeta?: McpToolUiMeta;
  /** Current theme */
  theme?: 'light' | 'dark';
  /** Callback when app requests to inject a message */
  onMessage?: (content: string) => void;
}

/** Default restrictive CSP for MCP Apps with no declared CSP. */
function buildCspMeta(uiMeta?: McpToolUiMeta): string {
  if (!uiMeta?.csp) {
    return `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; media-src data:; connect-src 'none';">`;
  }
  const csp = uiMeta.csp;
  const parts: string[] = ["default-src 'none'"];
  const connectSrc = csp.connect_domains?.length ? csp.connect_domains.join(' ') : "'none'";
  parts.push(`connect-src ${connectSrc}`);
  parts.push("script-src 'unsafe-inline'" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : ''));
  parts.push("style-src 'unsafe-inline'" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : ''));
  parts.push("img-src data:" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : ''));
  parts.push("media-src data:" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : ''));
  parts.push("font-src" + (csp.resource_domains?.length ? ' ' + csp.resource_domains.join(' ') : " 'none'"));
  if (csp.frame_domains?.length) {
    parts.push(`frame-src ${csp.frame_domains.join(' ')}`);
  }
  if (csp.base_uri_domains?.length) {
    parts.push(`base-uri ${csp.base_uri_domains.join(' ')}`);
  }
  return `<meta http-equiv="Content-Security-Policy" content="${parts.join('; ')}">`;
}

/** Build sandbox attribute string based on permissions. */
function buildSandbox(uiMeta?: McpToolUiMeta): string {
  // Always allow scripts; never allow same-origin
  const parts = ['allow-scripts'];
  // Future: add allow-camera etc. based on uiMeta.permissions
  return parts.join(' ');
}

export default function McpAppView(props: McpAppViewProps) {
  const [iframeHeight, setIframeHeight] = createSignal(300);
  let iframeRef: HTMLIFrameElement | undefined;
  let bridge: McpAppBridge | undefined;

  // Inject CSP meta tag into the app HTML
  const preparedHtml = () => {
    const cspTag = buildCspMeta(props.uiMeta);
    const html = props.html;
    // Insert CSP meta as the first child of <head>, or prepend if no <head>
    if (html.includes('<head>')) {
      return html.replace('<head>', `<head>${cspTag}`);
    } else if (html.includes('<html>')) {
      return html.replace('<html>', `<html><head>${cspTag}</head>`);
    }
    return `<head>${cspTag}</head>${html}`;
  };

  createEffect(() => {
    if (!iframeRef) return;

    bridge = new McpAppBridge({
      iframe: iframeRef,
      serverId: props.serverId,
      toolName: props.toolName,
      toolInput: props.toolInput,
      toolOutput: props.toolOutput,
      toolIsError: props.toolIsError,
      sessionId: props.sessionId,
      daemonUrl: props.daemonUrl,
      theme: props.theme ?? 'dark',
      onSizeChanged: (_w, h) => {
        if (h > 0 && h < 2000) setIframeHeight(h);
      },
      onMessage: props.onMessage,
    });
  });

  onCleanup(() => {
    bridge?.destroy();
  });

  const border = () => props.uiMeta?.prefers_border !== false;

  return (
    <div
      class="mcp-app-container mt-2 overflow-hidden rounded-lg"
      classList={{
        'border border-border': border(),
      }}
    >
      <div class="flex items-center gap-2 bg-muted/30 px-3 py-1.5 text-xs text-muted-foreground">
        <span class="font-medium">⚡ {props.toolName}</span>
        <span class="opacity-60">MCP App</span>
      </div>
      <iframe
        ref={iframeRef}
        srcdoc={preparedHtml()}
        sandbox={buildSandbox(props.uiMeta)}
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
