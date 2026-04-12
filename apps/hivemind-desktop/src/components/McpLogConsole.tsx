import { For, Show, createSignal, createEffect, onCleanup, createMemo } from 'solid-js';
import { listen } from '@tauri-apps/api/event';
import { Trash2, ArrowDownToLine, ChevronDown, ChevronUp } from 'lucide-solid';
import { authFetch } from '~/lib/authFetch';
import type { McpServerSnapshot, McpServerLog, McpServerLogEvent } from '~/types';

const MAX_LOCAL_LOGS = 500;

interface McpLogConsoleProps {
  session_id: string;
  daemon_url: string;
  servers: McpServerSnapshot[];
}

export default function McpLogConsole(props: McpLogConsoleProps) {
  const [logBuffers, setLogBuffers] = createSignal<Record<string, McpServerLog[]>>({});
  const [activeServer, setActiveServer] = createSignal<string | null>(null);
  const [autoScroll, setAutoScroll] = createSignal(true);
  const [collapsed, setCollapsed] = createSignal(false);
  const [panelHeight, setPanelHeight] = createSignal(280);

  let scrollRef: HTMLDivElement | undefined;

  const MIN_HEIGHT = 80;
  const MAX_HEIGHT_VH = 0.5;

  // Select first server tab automatically
  createEffect(() => {
    const svrs = props.servers;
    const current = activeServer();
    if (svrs.length > 0 && (!current || !svrs.find((s) => s.id === current))) {
      setActiveServer(svrs[0].id);
    }
  });

  // Fetch existing logs when switching tabs or on mount
  createEffect(() => {
    const server_id = activeServer();
    if (!server_id || !props.daemon_url || !props.session_id) return;
    void fetchLogsFor(server_id);
  });

  async function fetchLogsFor(server_id: string) {
    try {
      const resp = await authFetch(
        `${props.daemon_url}/api/v1/sessions/${props.session_id}/mcp/servers/${server_id}/logs`,
      );
      if (resp.ok) {
        const data: McpServerLog[] = await resp.json();
        setLogBuffers((prev) => ({ ...prev, [server_id]: data.slice(-MAX_LOCAL_LOGS) }));
        if (autoScroll()) scrollToBottom();
      }
    } catch {
      // Ignore fetch errors
    }
  }

  // Subscribe to realtime log events
  let disposed = false;
  const unlistenPromise = listen<string>('mcp:event', (event) => {
    if (disposed) return;
    try {
      const envelope = JSON.parse(event.payload);
      if (envelope.topic !== 'mcp.server.log') return;
      const logEvent: McpServerLogEvent = envelope.payload;
      if (!logEvent.server_id || !logEvent.log) return;

      setLogBuffers((prev) => {
        const existing = prev[logEvent.server_id] ?? [];
        const updated = [...existing, logEvent.log];
        return {
          ...prev,
          [logEvent.server_id]: updated.length > MAX_LOCAL_LOGS
            ? updated.slice(-MAX_LOCAL_LOGS)
            : updated,
        };
      });

      if (autoScroll() && activeServer() === logEvent.server_id) {
        requestAnimationFrame(() => scrollToBottom());
      }
    } catch {
      // Ignore parse errors
    }
  });

  onCleanup(() => {
    disposed = true;
    void unlistenPromise.then((fn) => fn());
  });

  function scrollToBottom() {
    if (scrollRef) {
      scrollRef.scrollTop = scrollRef.scrollHeight;
    }
  }

  function clearLogs() {
    const server_id = activeServer();
    if (server_id) {
      setLogBuffers((prev) => ({ ...prev, [server_id]: [] }));
    }
  }

  const currentLogs = createMemo(() => {
    const server_id = activeServer();
    if (!server_id) return [];
    return logBuffers()[server_id] ?? [];
  });

  function formatTimestamp(ms: number): string {
    if (!Number.isFinite(ms)) return '--:--:--.---';
    const d = new Date(ms);
    if (Number.isNaN(d.getTime())) return '--:--:--.---';
    return d.toLocaleTimeString([], { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' })
      + '.' + String(d.getMilliseconds()).padStart(3, '0');
  }

  function logMessageClass(msg: string): string {
    if (msg.startsWith('[stderr]')) return 'mcp-log-stderr';
    if (msg.startsWith('[mcp:error]') || msg.startsWith('[mcp:critical]') || msg.startsWith('[mcp:alert]') || msg.startsWith('[mcp:emergency]')) return 'mcp-log-error';
    if (msg.startsWith('[mcp:warning]')) return 'mcp-log-warning';
    if (msg.startsWith('[mcp:')) return 'mcp-log-mcp';
    if (msg.startsWith('error:')) return 'mcp-log-error';
    return 'mcp-log-lifecycle';
  }

  function statusDotClass(status: string): string {
    switch (status) {
      case 'connected': return 'mcp-console-tab-dot connected';
      case 'connecting': return 'mcp-console-tab-dot connecting';
      case 'error': return 'mcp-console-tab-dot error';
      default: return 'mcp-console-tab-dot disconnected';
    }
  }

  // Drag-to-resize handler
  const onSplitterPointerDown = (e: PointerEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = panelHeight();
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    document.body.style.cursor = 'row-resize';
    document.body.style.userSelect = 'none';

    const maxH = window.innerHeight * MAX_HEIGHT_VH;
    const onMove = (ev: PointerEvent) => {
      const newHeight = Math.min(maxH, Math.max(MIN_HEIGHT, startHeight - (ev.clientY - startY)));
      setPanelHeight(newHeight);
    };
    const onUp = () => {
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      target.releasePointerCapture(e.pointerId);
      target.removeEventListener('pointermove', onMove);
      target.removeEventListener('pointerup', onUp);
    };
    target.addEventListener('pointermove', onMove);
    target.addEventListener('pointerup', onUp);
  };

  return (
    <div class="mcp-log-console" style={collapsed() ? 'height:36px' : `height:${panelHeight()}px`}>
      {/* Resize handle */}
      <Show when={!collapsed()}>
        <div class="mcp-log-console-resize" onPointerDown={onSplitterPointerDown} />
      </Show>

      {/* Header bar */}
      <div class="mcp-log-console-header">
        <span class="mcp-log-console-title">MCP Logs</span>

        {/* Server tabs */}
        <div class="mcp-log-console-tabs">
          <For each={props.servers}>
            {(server) => (
              <button
                class={`mcp-log-console-tab ${activeServer() === server.id ? 'active' : ''}`}
                onClick={() => setActiveServer(server.id)}
                title={`${server.id} (${server.status})`}
              >
                <span class={statusDotClass(server.status)} />
                <span class="mcp-log-console-tab-label">{server.id}</span>
              </button>
            )}
          </For>
        </div>

        <div class="mcp-log-console-actions">
          <button
            class="mcp-log-console-action-btn"
            onClick={clearLogs}
            title="Clear logs"
          >
            <Trash2 size={13} />
          </button>
          <button
            class={`mcp-log-console-action-btn ${autoScroll() ? 'active' : ''}`}
            onClick={() => setAutoScroll(!autoScroll())}
            title={autoScroll() ? 'Auto-scroll ON' : 'Auto-scroll OFF'}
          >
            <ArrowDownToLine size={13} />
          </button>
          <button
            class="mcp-log-console-action-btn"
            onClick={() => setCollapsed(!collapsed())}
            title={collapsed() ? 'Expand' : 'Collapse'}
          >
            {collapsed() ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
          </button>
        </div>
      </div>

      {/* Log content */}
      <Show when={!collapsed()}>
        <div
          class="mcp-log-console-content"
          ref={scrollRef}
        >
          <Show
            when={currentLogs().length > 0}
            fallback={
              <div class="mcp-log-console-empty">
                {activeServer() ? 'No log entries yet' : 'Select a server tab to view logs'}
              </div>
            }
          >
            <For each={currentLogs()}>
              {(entry) => (
                <div class="mcp-log-console-entry">
                  <span class="mcp-log-console-time">{formatTimestamp(entry.timestamp_ms)}</span>
                  <span class={`mcp-log-console-msg ${logMessageClass(entry.message)}`}>{entry.message}</span>
                </div>
              )}
            </For>
          </Show>
        </div>
      </Show>
    </div>
  );
}
