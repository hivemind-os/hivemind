import { createSignal, createEffect, on, onCleanup, For, Show, type Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Terminal, Square, ChevronRight, Clock, Cpu } from 'lucide-solid';
import { AnsiUp } from 'ansi_up';
import DOMPurify from 'dompurify';

interface ProcessOwner {
  kind: 'session' | 'unknown';
  session_id?: string;
}

interface ProcessStatus {
  state: 'running' | 'exited' | 'killed' | 'failed';
  code?: number;
  error?: string;
}

interface ProcessInfo {
  id: string;
  pid: number;
  command: string;
  working_dir: string | null;
  status: ProcessStatus;
  uptime_secs: number;
  owner: ProcessOwner;
}

interface ProcessListResponse {
  processes: ProcessInfo[];
}

interface ProcessStatusResponse {
  info: ProcessInfo;
  output: string;
}

export interface SessionProcessesProps {
  session_id: Accessor<string | null>;
  daemonOnline: Accessor<boolean>;
}

function statusPill(status: ProcessStatus): { cls: string; label: string } {
  switch (status.state) {
    case 'running': return { cls: 'pill info', label: 'running' };
    case 'exited': return {
      cls: status.code === 0 ? 'pill success' : 'pill danger',
      label: `exited (${status.code ?? '?'})`,
    };
    case 'killed': return { cls: 'pill warning', label: 'killed' };
    case 'failed': return { cls: 'pill danger', label: 'failed' };
    default: return { cls: 'pill neutral', label: status.state };
  }
}

function uptimeStr(secs: number): string {
  if (secs < 60) return `${Math.floor(secs)}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${Math.floor(secs % 60)}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

function truncateCmd(cmd: string, max = 80): string {
  return cmd.length > max ? cmd.slice(0, max) + '…' : cmd;
}

const ansi = new AnsiUp();
ansi.use_classes = true;

export default function SessionProcesses(props: SessionProcessesProps) {
  const [processes, setProcesses] = createSignal<ProcessInfo[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [expandedId, setExpandedId] = createSignal<string | null>(null);
  const [expandedOutput, setExpandedOutput] = createSignal('');
  const [outputLoading, setOutputLoading] = createSignal(false);

  async function loadProcesses(sid: string) {
    setLoading(true);
    try {
      const resp = await invoke<ProcessListResponse>('list_session_processes', { session_id: sid });
      setProcesses(resp.processes ?? []);
    } catch {
      setProcesses([]);
    } finally {
      setLoading(false);
    }
  }

  async function loadOutput(processId: string) {
    setOutputLoading(true);
    try {
      const resp = await invoke<ProcessStatusResponse>('get_process_status', {
        process_id: processId, tail_lines: 50,
      });
      setExpandedOutput(resp.output ?? '');
    } catch {
      setExpandedOutput('(unable to fetch output)');
    } finally {
      setOutputLoading(false);
    }
  }

  async function handleKill(processId: string) {
    try {
      await invoke('kill_process', { process_id: processId });
      const sid = props.session_id();
      if (sid) await loadProcesses(sid);
    } catch (e) {
      console.warn('kill_process failed', e);
    }
  }

  function toggleExpand(processId: string) {
    if (expandedId() === processId) {
      setExpandedId(null);
      setExpandedOutput('');
    } else {
      setExpandedId(processId);
      void loadOutput(processId);
    }
  }

  createEffect(on(
    () => [props.session_id(), props.daemonOnline()] as const,
    ([sid, online]) => {
      let unlisten: UnlistenFn | undefined;
      let disposed = false;
      let timer: ReturnType<typeof setTimeout> | undefined;

      if (sid && online) {
        // Initial load.
        void loadProcesses(sid);

        // Start SSE subscription.
        void invoke('process_subscribe_events', { session_id: sid });

        const debouncedRefresh = () => {
          if (timer) clearTimeout(timer);
          timer = setTimeout(() => {
            if (disposed) return;
            const currentSid = props.session_id();
            if (currentSid && props.daemonOnline()) {
              void loadProcesses(currentSid);
              const eid = expandedId();
              if (eid) void loadOutput(eid);
            }
          }, 300);
        };

        // Listen for process lifecycle events and refetch the list.
        void listen('process:event', () => {
          debouncedRefresh();
        }).then((fn) => {
          if (disposed) { fn(); return; }
          unlisten = fn;
        });
      } else {
        setProcesses([]);
      }

      onCleanup(() => {
        disposed = true;
        if (timer) clearTimeout(timer);
        if (unlisten) { unlisten(); unlisten = undefined; }
      });
    },
  ));

  return (
    <div class="flex flex-1 flex-col" style="padding:12px 16px;overflow-y:auto;">
      <Show when={processes().length === 0}>
        <div class="flex flex-col items-center justify-center flex-1 gap-3 text-muted-foreground" style="padding:40px 20px;">
          <Terminal size={32} style="opacity:0.3;" />
          <p style="margin:0;font-size:0.9em;">No background processes</p>
          <p style="margin:0;font-size:0.78em;opacity:0.7;">Processes started by agents will appear here.</p>
        </div>
      </Show>

      <Show when={processes().length > 0}>
        <div style="display:flex;flex-direction:column;gap:8px;padding-top:4px;">
          <For each={processes()}>
            {(proc) => {
              const pill = () => statusPill(proc.status);
              const isExpanded = () => expandedId() === proc.id;
              const isRunning = () => proc.status.state === 'running';

              return (
                <div
                  style={`
                    border:1px solid hsl(var(--border));
                    border-radius:8px;
                    background:hsl(var(--card));
                    overflow:hidden;
                    transition:border-color 0.15s;
                  `}
                >
                  {/* Summary row */}
                  <div
                    style="display:flex;align-items:center;gap:10px;padding:10px 14px;cursor:pointer;"
                    onClick={() => toggleExpand(proc.id)}
                  >
                    <Terminal size={14} style={`flex-shrink:0;opacity:0.5;color:${isRunning() ? 'hsl(var(--primary))' : 'inherit'}`} />

                    <div style="flex:1;min-width:0;">
                      <div style="display:flex;align-items:center;gap:8px;">
                        <span style="font-size:0.84em;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;font-family:monospace;">
                          {truncateCmd(proc.command)}
                        </span>
                        <span class={pill().cls} style="font-size:0.68em;flex-shrink:0;">{pill().label}</span>
                      </div>
                      <div style="display:flex;align-items:center;gap:12px;font-size:0.75em;color:hsl(var(--muted-foreground));margin-top:2px;">
                        <span style="display:inline-flex;align-items:center;gap:3px;">
                          <Cpu size={10} /> PID {proc.pid}
                        </span>
                        <span style="display:inline-flex;align-items:center;gap:3px;">
                          <Clock size={10} /> {uptimeStr(proc.uptime_secs)}
                        </span>
                      </div>
                    </div>

                    {/* Actions */}
                    <div style="display:flex;align-items:center;gap:6px;flex-shrink:0;" onClick={(e: Event) => e.stopPropagation()}>
                      <Show when={isRunning()}>
                        <button
                          class="icon-btn"
                          title="Kill process"
                          style="color:hsl(var(--destructive));"
                          onClick={() => handleKill(proc.id)}
                        >
                          <Square size={14} />
                        </button>
                      </Show>
                    </div>

                    <ChevronRight size={14} style={`flex-shrink:0;opacity:0.4;transition:transform 0.15s;${isExpanded() ? 'transform:rotate(90deg);' : ''}`} />
                  </div>

                  {/* Expanded output */}
                  <Show when={isExpanded()}>
                    <div style="border-top:1px solid hsl(var(--border));padding:10px 14px;">
                      <Show when={proc.working_dir}>
                        <div style="font-size:0.75em;color:hsl(var(--muted-foreground));margin-bottom:6px;">
                          <span style="opacity:0.7;">cwd:</span> {proc.working_dir}
                        </div>
                      </Show>
                      <div style="font-size:0.75em;color:hsl(var(--muted-foreground));margin-bottom:6px;">
                        <span style="opacity:0.7;">command:</span>{' '}
                        <code style="font-family:monospace;color:hsl(var(--foreground));">{proc.command}</code>
                      </div>
                      <Show when={proc.status.state === 'failed' && proc.status.error}>
                        <div style="margin-bottom:6px;padding:6px 8px;border-radius:4px;background:hsl(var(--destructive) / 0.1);color:hsl(var(--destructive));font-size:0.8em;">
                          {proc.status.error}
                        </div>
                      </Show>
                      <div style="position:relative;">
                        <pre
                          style={`
                            margin:0;
                            padding:10px;
                            border-radius:6px;
                            background:hsl(var(--muted));
                            color:hsl(var(--foreground));
                            font-size:0.78em;
                            line-height:1.4;
                            max-height:200px;
                            overflow:auto;
                            white-space:pre-wrap;
                            word-break:break-all;
                            font-family:monospace;
                          `}
                          innerHTML={
                            outputLoading() && !expandedOutput()
                              ? 'Loading…'
                              : expandedOutput()
                                ? DOMPurify.sanitize(ansi.ansi_to_html(expandedOutput()))
                                : '(no output)'
                          }
                        />
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
  );
}
