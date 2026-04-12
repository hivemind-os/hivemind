import { Show, For, Index, createSignal, createEffect, onMount } from 'solid-js';
import type { Accessor, Setter, JSX } from 'solid-js';
import { authFetch } from '~/lib/authFetch';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import type { HiveMindConfigData } from '../../types';

export interface RuntimeTabProps {
  editConfig: Accessor<HiveMindConfigData | null>;
  setEditConfig: Setter<HiveMindConfigData | null>;
  configLoadingFallback: (label: string) => JSX.Element;
  tab: 'python' | 'node';
}

const RuntimeTab = (props: RuntimeTabProps) => {
  const editConfig = props.editConfig;
  const setEditConfig = props.setEditConfig;

  // ── Python environment state ──
  const py = () => editConfig()?.python ?? { enabled: true, python_version: '3.12', base_packages: [], auto_detect_workspace_deps: true, uv_version: '0.6.14' };
  const updatePython = (field: string, value: unknown) => {
    setEditConfig((c) => c ? {
      ...c,
      python: { ...py(), [field]: value },
    } : null);
  };
  const addPackage = () => {
    updatePython('base_packages', [...(py().base_packages ?? []), '']);
  };
  const removePackage = (idx: number) => {
    const pkgs = [...(py().base_packages ?? [])];
    pkgs.splice(idx, 1);
    updatePython('base_packages', pkgs);
  };
  const updatePackage = (idx: number, value: string) => {
    const pkgs = [...(py().base_packages ?? [])];
    pkgs[idx] = value;
    updatePython('base_packages', pkgs);
  };

  const [pyStatus, setPyStatus] = createSignal<{ phase: string; message: string } | null>(null);
  const [reinstalling, setReinstalling] = createSignal(false);

  const fetchStatus = async () => {
    try {
      const res = await authFetch('/api/v1/python/status');
      if (res.ok) setPyStatus(await res.json());
    } catch { /* ignore */ }
  };
  onMount(() => { fetchStatus(); });

  const reinstall= async () => {
    setReinstalling(true);
    try {
      const res = await authFetch('/api/v1/python/reinstall', { method: 'POST' });
      if (res.ok) setPyStatus(await res.json());
    } catch { /* ignore */ }
    setReinstalling(false);
  };

  // ── Node.js environment state ──
  const nd = () => editConfig()?.node ?? { enabled: true, node_version: '22.16.0' };
  const updateNode = (field: string, value: unknown) => {
    setEditConfig((c) => c ? {
      ...c,
      node: { ...nd(), [field]: value },
    } : null);
  };

  const [nodeStatus, setNodeStatus] = createSignal<{ status: string; node_dir?: string; progress?: string; error?: string } | null>(null);
  const [nodeReinstalling, setNodeReinstalling] = createSignal(false);

  const fetchNodeStatus = async () => {
    try {
      const res = await authFetch('/api/v1/node/status');
      if (res.ok) setNodeStatus(await res.json());
    } catch { /* ignore */ }
  };
  onMount(() => { fetchNodeStatus(); });

  const reinstallNode= async () => {
    setNodeReinstalling(true);
    try {
      const res = await authFetch('/api/v1/node/reinstall', { method: 'POST' });
      if (res.ok) setNodeStatus(await res.json());
    } catch { /* ignore */ }
    setNodeReinstalling(false);
  };

  return (
    <>
      <Show when={props.tab === 'python'}>
        <div class="settings-section">
          <Show when={editConfig()} fallback={props.configLoadingFallback('Python settings')}>
                <>
                  <h3>Managed Python Environment</h3>
                  <p class="muted" style="font-size:12px;margin-bottom:8px;">
                    A curated Python environment managed via <code>uv</code>. Shell commands automatically use this environment.
                  </p>

                  <Show when={pyStatus()}>
                    {(status) => (
                      <div style="margin-bottom:12px;padding:8px 12px;border-radius:6px;background:var(--bg-secondary);font-size:12px;">
                        <strong>Status:</strong> {status().phase}
                        <Show when={status().message}>
                          <span class="muted"> — {status().message}</span>
                        </Show>
                      </div>
                    )}
                  </Show>

                  <div class="settings-form">
                    <Switch checked={py().enabled} onChange={(checked) => updatePython('enabled', checked)} class="flex items-center gap-2">
                      <SwitchControl><SwitchThumb /></SwitchControl>
                      <SwitchLabel>Enable managed Python environment</SwitchLabel>
                    </Switch>
                    <label>
                      <span>Python version</span>
                      <input type="text" value={py().python_version}
                        onChange={(e) => updatePython('python_version', e.currentTarget.value)} />
                    </label>
                    <label>
                      <span>uv version</span>
                      <input type="text" value={py().uv_version}
                        onChange={(e) => updatePython('uv_version', e.currentTarget.value)} />
                    </label>
                    <Switch checked={py().auto_detect_workspace_deps} onChange={(checked) => updatePython('auto_detect_workspace_deps', checked)} class="flex items-center gap-2">
                      <SwitchControl><SwitchThumb /></SwitchControl>
                      <SwitchLabel>Auto-detect workspace dependencies (requirements.txt, pyproject.toml)</SwitchLabel>
                    </Switch>
                  </div>

                  <h3 style="margin-top: 1.5rem;">Base Packages</h3>
                  <p class="muted" style="font-size:12px;margin-bottom:8px;">
                    Packages pre-installed in every managed environment. Changes take effect after reinstall.
                  </p>
                  <div style="display:flex;flex-direction:column;gap:4px;">
                    <Index each={py().base_packages ?? []}>
                      {(pkg, i) => (
                        <div style="display:flex;gap:4px;align-items:center;">
                          <input type="text" value={pkg()} style="flex:1"
                            placeholder="package name"
                            onChange={(e) => updatePackage(i, e.currentTarget.value)} />
                          <button style="padding:2px 6px;font-size:12px;" onClick={() => removePackage(i)}>✕</button>
                        </div>
                      )}
                    </Index>
                    <button style="font-size:12px;align-self:flex-start;" onClick={addPackage}>+ Add package</button>
                  </div>

                  <div style="margin-top:1.5rem;">
                    <button
                      disabled={reinstalling()}
                      onClick={reinstall}
                      style="font-size:13px;"
                    >
                      {reinstalling() ? 'Reinstalling…' : 'Reinstall Environment'}
                    </button>
                    <span class="muted" style="font-size:11px;margin-left:8px;">
                      Rebuilds the venv with current settings
                    </span>
                  </div>
                </>
          </Show>
        </div>
      </Show>

      <Show when={props.tab === 'node'}>
        <div class="settings-section">
          <Show when={editConfig()} fallback={props.configLoadingFallback('Node.js settings')}>
                <>
                  <h3>Managed Node.js Environment</h3>
                  <p class="muted" style="font-size:12px;margin-bottom:8px;">
                    A managed Node.js runtime for MCP servers. When enabled, <code>npx</code>, <code>npm</code>, and <code>node</code> commands will use this installation automatically.
                  </p>

                  <Show when={nodeStatus()}>
                    {(status) => (
                      <div style="margin-bottom:12px;padding:8px 12px;border-radius:6px;background:var(--bg-secondary);font-size:12px;">
                        <strong>Status:</strong> {status().status}
                        <Show when={status().progress}>
                          <span class="muted"> — {status().progress}</span>
                        </Show>
                        <Show when={status().error}>
                          <span style="color:var(--color-error)"> — {status().error}</span>
                        </Show>
                        <Show when={status().node_dir}>
                          <span class="muted"> — {status().node_dir}</span>
                        </Show>
                      </div>
                    )}
                  </Show>

                  <div class="settings-form">
                    <Switch checked={nd().enabled} onChange={(checked) => updateNode('enabled', checked)} class="flex items-center gap-2">
                      <SwitchControl><SwitchThumb /></SwitchControl>
                      <SwitchLabel>Enable managed Node.js environment</SwitchLabel>
                    </Switch>
                    <label>
                      <span>Node.js version</span>
                      <input type="text" value={nd().node_version}
                        onChange={(e) => updateNode('node_version', e.currentTarget.value)} />
                    </label>
                  </div>

                  <div style="margin-top:1.5rem;">
                    <button
                      disabled={nodeReinstalling()}
                      onClick={reinstallNode}
                      style="font-size:13px;"
                    >
                      {nodeReinstalling() ? 'Reinstalling…' : 'Reinstall Node.js'}
                    </button>
                    <span class="muted" style="font-size:11px;margin-left:8px;">
                      Re-downloads Node.js with current version settings
                    </span>
                  </div>
                </>
          </Show>
        </div>
      </Show>
    </>
  );
};

export default RuntimeTab;
