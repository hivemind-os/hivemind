import { Show, For, createSignal, createEffect } from 'solid-js';
import type { Accessor, JSX } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { ClipboardList, FlaskConical, Trash2 } from 'lucide-solid';
import type { HiveMindConfigData } from '../../types';

export interface RecordingsTabProps {
  editConfig: Accessor<HiveMindConfigData | null>;
  daemonOnline: Accessor<boolean>;
  active: Accessor<boolean>;
  configLoadingFallback: (label: string) => JSX.Element;
}

const RecordingsTab = (props: RecordingsTabProps) => {
  const editConfig = props.editConfig;

  const [recordingId, setRecordingId] = createSignal<string | null>(null);
  const [recordingName, setRecordingName] = createSignal('');
  const [recordingTopicFilter, setRecordingTopicFilter] = createSignal('');
  const [recordings, setRecordings] = createSignal<Array<{ id: string; name: string | null; started_ms: number; stopped_ms: number | null; event_count: number }>>([]);
  const [recordingError, setRecordingError] = createSignal<string | null>(null);

  const loadRecordings = async () => {
    try {
      const list = await invoke('event_recording_list') as Array<{ id: string; name: string | null; started_ms: number; stopped_ms: number | null; event_count: number }>;
      setRecordings(list);
      const active = list.find(r => r.stopped_ms === null);
      setRecordingId(active?.id ?? null);
    } catch (_) { /* daemon may be offline */ }
  };

  const startRecording = async () => {
    setRecordingError(null);
    try {
      const result = await invoke('event_recording_start', {
        name: recordingName() || null,
        topic_filter: recordingTopicFilter() || null,
      }) as { recording_id: string };
      setRecordingId(result.recording_id);
      setRecordingName('');
      setRecordingTopicFilter('');
      await loadRecordings();
    } catch (e) {
      setRecordingError(String(e));
    }
  };

  const stopRecording = async () => {
    setRecordingError(null);
    const id = recordingId();
    if (!id) return;
    try {
      await invoke('event_recording_stop', { recording_id: id });
      setRecordingId(null);
      await loadRecordings();
    } catch (e) {
      setRecordingError(String(e));
    }
  };

  const exportRecording = async (id: string, format: string) => {
    try {
      const content = await invoke('event_recording_export', { recording_id: id, format }) as string;
      const ext = format === 'rust_test' ? 'rs' : 'json';
      const blob = new Blob([content], { type: 'text/plain' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `recording-${id}.${ext}`;
      a.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      setRecordingError(String(e));
    }
  };

  const deleteRecording = async (id: string) => {
    try {
      await invoke('event_recording_delete', { recording_id: id });
      await loadRecordings();
    } catch (e) {
      setRecordingError(String(e));
    }
  };

  // Load recordings when tab becomes active and daemon is online
  createEffect(() => {
    if (props.active() && props.daemonOnline()) {
      void loadRecordings();
    }
  });

  return (
    <div class="settings-section">
      <h3>Event Recording</h3>
      <Show when={recordingError()}>
        <p class="muted text-destructive">{recordingError()}</p>
      </Show>

      <Show when={editConfig()} fallback={props.configLoadingFallback('configuration')}>
          <>
            <Show when={!recordingId()} fallback={
              <div class="settings-form">
                <p><span class="pill processing" style="display: inline-flex; align-items: center; gap: 4px;">
                  <span style="display:inline-block;width:8px;height:8px;border-radius:50%;background:hsl(var(--destructive));animation:pulse 1s infinite;" />
                  Recording…
                </span></p>
                <button class="primary" onClick={() => void stopRecording()}>Stop Recording</button>
              </div>
            }>
                <div class="settings-form">
                  <label>
                    <span>Name (optional)</span>
                    <input type="text" value={recordingName()} placeholder="e.g. MCP flow test"
                      onChange={(e) => setRecordingName(e.currentTarget.value)} />
                  </label>
                  <label>
                    <span>Topic filter (optional)</span>
                    <input type="text" value={recordingTopicFilter()} placeholder="e.g. chat. or mcp."
                      onChange={(e) => setRecordingTopicFilter(e.currentTarget.value)} />
                  </label>
                  <button class="primary" onClick={() => void startRecording()}>Start Recording</button>
                </div>
            </Show>

            <Show when={recordings().length > 0}>
              <h4 style="margin-top: 12px;">Past Recordings</h4>
              <table class="mini-table" style="width: 100%; font-size: 0.85em;">
                <thead>
                  <tr><th>Name</th><th>Events</th><th>Status</th><th>Actions</th></tr>
                </thead>
                <tbody>
                  <For each={recordings()}>
                    {(rec) => (
                      <tr>
                        <td>{rec.name ?? rec.id.slice(0, 8)}</td>
                        <td>{rec.event_count}</td>
                        <td><span class={`pill ${rec.stopped_ms ? 'neutral' : 'processing'}`}>{rec.stopped_ms ? 'Done' : 'Recording'}</span></td>
                        <td style="display: flex; gap: 4px;">
                          <Show when={rec.stopped_ms}>
                            <>
                              <button onClick={() => void exportRecording(rec.id, 'json')} title="Export JSON"><ClipboardList size={14} /></button>
                              <button onClick={() => void exportRecording(rec.id, 'rust_test')} title="Export Rust test"><FlaskConical size={14} /></button>
                              <button onClick={() => void deleteRecording(rec.id)} title="Delete"><Trash2 size={14} /></button>
                            </>
                          </Show>
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </Show>
          </>
      </Show>
    </div>
  );
};

export default RecordingsTab;
