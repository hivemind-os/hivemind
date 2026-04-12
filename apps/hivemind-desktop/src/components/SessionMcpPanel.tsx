import { For, Show, createSignal, createEffect, onCleanup } from 'solid-js';
import { Plug, Link, Unlink, RefreshCw, ChevronDown, ChevronRight, Shield, ShieldOff } from 'lucide-solid';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { authFetch } from '~/lib/authFetch';
import McpLogConsole from './McpLogConsole';
import type { McpServerSnapshot } from '~/types';

interface SessionMcpPanelProps {
  session_id: string;
  daemon_url: string;
}

export default function SessionMcpPanel(props: SessionMcpPanelProps) {
  const [servers, setServers] = createSignal<McpServerSnapshot[]>([]);
  const [expandedServers, setExpandedServers] = createSignal<Set<string>>(new Set());
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  let fetchServersSeq = 0;
  async function fetchServers() {
    if (!props.daemon_url || !props.session_id) return;
    const mySeq = ++fetchServersSeq;
    setLoading(true);
    setError(null);
    try {
      const resp = await authFetch(
        `${props.daemon_url}/api/v1/sessions/${props.session_id}/mcp/servers`,
      );
      if (mySeq !== fetchServersSeq) return;
      if (resp.ok) {
        setServers(await resp.json());
      } else {
        setError(`Failed to fetch servers: ${resp.statusText}`);
      }
    } catch (e: any) {
      if (mySeq !== fetchServersSeq) return;
      setError(`Failed to fetch servers: ${e.message}`);
    } finally {
      if (mySeq === fetchServersSeq) setLoading(false);
    }
  }

  async function connectServer(server_id: string) {
    if (!props.daemon_url || !props.session_id) return;
    try {
      const resp = await authFetch(
        `${props.daemon_url}/api/v1/sessions/${props.session_id}/mcp/servers/${server_id}/connect`,
        { method: 'POST' },
      );
      if (!resp.ok) {
        const text = await resp.text().catch(() => resp.statusText);
        setError(`Connect failed: ${text}`);
      }
      await fetchServers();
    } catch (e: any) {
      setError(`Failed to connect MCP server: ${e.message}`);
    }
  }

  async function disconnectServer(server_id: string) {
    if (!props.daemon_url || !props.session_id) return;
    try {
      const resp = await authFetch(
        `${props.daemon_url}/api/v1/sessions/${props.session_id}/mcp/servers/${server_id}/disconnect`,
        { method: 'POST' },
      );
      if (!resp.ok) {
        const text = await resp.text().catch(() => resp.statusText);
        setError(`Disconnect failed: ${text}`);
      }
      await fetchServers();
    } catch (e: any) {
      setError(`Failed to disconnect MCP server: ${e.message}`);
    }
  }

  const [installingRuntimes, setInstallingRuntimes] = createSignal<Set<string>>(new Set());

  async function installRuntime(server_id: string) {
    if (!props.daemon_url) return;
    setInstallingRuntimes((prev) => new Set([...prev, server_id]));
    try {
      const resp = await authFetch(
        `${props.daemon_url}/api/v1/mcp/servers/${server_id}/install-runtime`,
        { method: 'POST' },
      );
      if (!resp.ok) {
        const text = await resp.text();
        console.error('Failed to install runtime:', text);
      }
      await fetchServers();
    } catch (e: any) {
      console.error('Failed to install runtime', e);
    } finally {
      setInstallingRuntimes((prev) => {
        const next = new Set(prev);
        next.delete(server_id);
        return next;
      });
    }
  }

  function isRuntimeError(error: string | null): boolean {
    return !!error && error.includes('which is not installed');
  }

  function canAutoInstallRuntime(error: string | null): boolean {
    if (!error) return false;
    return isRuntimeError(error) && (error.includes('Node.js') || error.includes('Python/uv'));
  }

  function toggleExpand(server_id: string) {
    setExpandedServers((prev) => {
      const next = new Set(prev);
      if (next.has(server_id)) {
        next.delete(server_id);
      } else {
        next.add(server_id);
      }
      return next;
    });
  }

  // Auto-refresh on mount and via push events
  createEffect(() => {
    const _sid = props.session_id;
    if (_sid) fetchServers();
  });

  // Subscribe to MCP SSE events and refetch on change (debounced)
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;
  const debouncedFetch = () => {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      if (!disposed && props.session_id) fetchServers();
    }, 300);
  };

  invoke('mcp_subscribe_events').catch((e: any) =>
    console.warn('Failed to start MCP event subscription', e),
  );

  const unlistenPromise = listen('mcp:event', (event) => {
    try {
      const envelope = JSON.parse(event.payload as string);
      // Only refetch server list for non-log events
      if (envelope.topic !== 'mcp.server.log') {
        debouncedFetch();
      }
    } catch {
      debouncedFetch();
    }
  });
  onCleanup(() => {
    disposed = true;
    if (debounceTimer) clearTimeout(debounceTimer);
    void unlistenPromise.then((fn) => fn());
  });

  function statusDotClass(status: string): string {
    switch (status) {
      case 'connected': return 'session-mcp-dot connected';
      case 'connecting': return 'session-mcp-dot connecting';
      case 'error': return 'session-mcp-dot error';
      default: return 'session-mcp-dot disconnected';
    }
  }

  return (
    <div class="session-mcp-panel">
      {/* Server list (top section) */}
      <div class="session-mcp-servers-section">
        <div class="session-mcp-header">
          <Plug size={16} />
          <span class="session-mcp-title">MCP Servers</span>
          <button
            class="session-mcp-refresh-btn"
            onClick={() => fetchServers()}
            title="Refresh"
            disabled={loading()}
          >
            <RefreshCw size={14} class={loading() ? 'spin' : ''} />
          </button>
        </div>

        <Show when={error()}>
          <div class="session-mcp-error">{error()}</div>
        </Show>

        <Show
          when={servers().length > 0}
          fallback={
            <div class="session-mcp-empty">
              {loading() ? 'Loading…' : 'No MCP servers configured'}
            </div>
          }
        >
          <div class="session-mcp-list">
            <For each={servers()}>
              {(server) => {
                const expanded = () => expandedServers().has(server.id);
                return (
                  <div class="session-mcp-server">
                    <div class="session-mcp-server-header" onClick={() => toggleExpand(server.id)}>
                      <span class={statusDotClass(server.status)} />
                      <span class="session-mcp-server-name">{server.id}</span>
                      <span class="session-mcp-server-transport">{server.transport}</span>
                      <span class={`session-mcp-server-status ${server.status}`}>
                        {server.status}
                      </span>
                      <Show when={server.sandbox_status}>
                        {(sb) => (
                          <span
                            style={`font-size:11px;margin-left:4px;display:inline-flex;align-items:center;gap:2px;color:${sb().active ? 'var(--color-success,#22c55e)' : 'hsl(var(--muted-foreground))'};`}
                            title={sb().active ? `Sandboxed (${sb().source})` : 'Not sandboxed'}
                          >
                            {sb().active ? <Shield size={12} /> : <ShieldOff size={12} />}
                            {sb().active ? 'sandboxed' : ''}
                          </span>
                        )}
                      </Show>
                      <Show when={server.status === 'error' && isRuntimeError(server.last_error)}>
                        <span style="font-size:11px;color:var(--color-warning,#f59e0b);margin-left:4px;" title={server.last_error ?? ''}>
                          ⚠ runtime
                        </span>
                      </Show>
                      <span class="session-mcp-expand-icon">
                        {expanded() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                      </span>
                    </div>

                    <Show when={expanded()}>
                      <div class="session-mcp-server-details">
                        <div class="session-mcp-server-stats">
                          <span><strong>Tools:</strong> {server.tool_count}</span>
                          <span><strong>Resources:</strong> {server.resource_count}</span>
                          <span><strong>Prompts:</strong> {server.prompt_count}</span>
                        </div>

                        <Show when={server.sandbox_status}>
                          {(sb) => (
                            <div style="font-size: 12px; margin-top: 6px; padding: 6px 8px; border-radius: 4px; background: hsl(var(--muted) / 0.3);">
                              <div style="display: flex; align-items: center; gap: 4px; margin-bottom: 4px;">
                                {sb().active ? <Shield size={12} /> : <ShieldOff size={12} />}
                                <strong>{sb().active ? 'Sandbox Active' : 'Sandbox Inactive'}</strong>
                                <span style="color: hsl(var(--muted-foreground)); margin-left: 4px;">({sb().source})</span>
                              </div>
                              <Show when={sb().active}>
                                <div style="display: flex; flex-wrap: wrap; gap: 6px 12px; color: hsl(var(--muted-foreground));">
                                  <span>Network: {sb().allow_network ? '✓' : '✕'}</span>
                                  <span>Read workspace: {sb().read_workspace ? '✓' : '✕'}</span>
                                  <span>Write workspace: {sb().write_workspace ? '✓' : '✕'}</span>
                                  <Show when={sb().extra_read_paths.length > 0}>
                                    <span>+{sb().extra_read_paths.length} read path{sb().extra_read_paths.length !== 1 ? 's' : ''}</span>
                                  </Show>
                                  <Show when={sb().extra_write_paths.length > 0}>
                                    <span>+{sb().extra_write_paths.length} write path{sb().extra_write_paths.length !== 1 ? 's' : ''}</span>
                                  </Show>
                                </div>
                              </Show>
                            </div>
                          )}
                        </Show>

                        <Show when={server.last_error}>
                          <div class="session-mcp-server-error">
                            <strong>Error:</strong> {server.last_error}
                            <Show when={canAutoInstallRuntime(server.last_error)}>
                              <div style="margin-top:6px;">
                                <button
                                  class="session-mcp-action-btn"
                                  disabled={installingRuntimes().has(server.id)}
                                  onClick={(e) => { e.stopPropagation(); installRuntime(server.id); }}
                                  style="background:var(--color-primary,#3b82f6);color:white;border:none;padding:4px 10px;border-radius:4px;font-size:12px;cursor:pointer;"
                                >
                                  {installingRuntimes().has(server.id) ? 'Installing…' : '⬇ Install Runtime'}
                                </button>
                              </div>
                            </Show>
                          </div>
                        </Show>

                        <div class="session-mcp-actions">
                          <Show when={server.status === 'disconnected' || server.status === 'error'}>
                            <button
                              class="session-mcp-action-btn"
                              onClick={(e) => { e.stopPropagation(); connectServer(server.id); }}
                            >
                              <Link size={12} /> Connect
                            </button>
                          </Show>
                          <Show when={server.status === 'connected'}>
                            <button
                              class="session-mcp-action-btn"
                              onClick={(e) => { e.stopPropagation(); disconnectServer(server.id); }}
                            >
                              <Unlink size={12} /> Disconnect
                            </button>
                          </Show>
                        </div>
                      </div>
                    </Show>
                  </div>
                );
              }}
            </For>
          </div>
        </Show>
      </div>

      {/* Log console (bottom section) */}
      <McpLogConsole
        session_id={props.session_id}
        daemon_url={props.daemon_url}
        servers={servers()}
      />
    </div>
  );
}
