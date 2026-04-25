import { Show } from 'solid-js';
import type { Accessor, Setter, JSX } from 'solid-js';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import type { HiveMindConfigData } from '../../types';

export interface CodeActTabProps {
  editConfig: Accessor<HiveMindConfigData | null>;
  setEditConfig: Setter<HiveMindConfigData | null>;
  configLoadingFallback: (label: string) => JSX.Element;
}

const CodeActTab = (props: CodeActTabProps) => {
  const editConfig = props.editConfig;
  const setEditConfig = props.setEditConfig;

  const ca = () => editConfig()?.code_act ?? {
    enabled: true,
    execution_timeout_secs: 30,
    max_output_bytes: 1048576,
    idle_timeout_secs: 600,
    max_sessions: 3,
    allow_network: false,
  };

  const update = (field: string, value: unknown) => {
    setEditConfig((c) => c ? {
      ...c,
      code_act: { ...ca(), [field]: value },
    } : null);
  };

  return (
    <div class="settings-section">
      <Show when={editConfig()} fallback={props.configLoadingFallback('CodeAct settings')}>
        <>
          <h3>CodeAct Sandbox</h3>
          <p class="muted" style="font-size:12px;margin-bottom:8px;">
            CodeAct lets the LLM write and execute Python code directly instead of
            using structured tool calls. Code runs in a persistent interpreter session
            that survives across turns within a conversation.
          </p>

          <div class="settings-form">
            <Switch checked={ca().enabled} onChange={(checked) => update('enabled', checked)} class="flex items-center gap-2">
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>Enable CodeAct code execution</SwitchLabel>
            </Switch>
          </div>

          <h3 style="margin-top: 1.5rem;">Resource Limits</h3>
          <p class="muted" style="font-size:12px;margin-bottom:8px;">
            Control how much time and output a single code block execution can consume.
          </p>
          <div class="settings-form">
            <label>
              <span>Execution timeout (seconds)</span>
              <input
                type="number"
                min="5"
                max="300"
                value={ca().execution_timeout_secs}
                onChange={(e) => update('execution_timeout_secs', Math.max(5, parseInt(e.currentTarget.value) || 30))}
              />
            </label>
            <label>
              <span>Max output size (bytes)</span>
              <input
                type="number"
                min="1024"
                value={ca().max_output_bytes}
                onChange={(e) => update('max_output_bytes', Math.max(1024, parseInt(e.currentTarget.value) || 1048576))}
              />
            </label>
          </div>

          <h3 style="margin-top: 1.5rem;">Session Lifecycle</h3>
          <p class="muted" style="font-size:12px;margin-bottom:8px;">
            The Python interpreter persists across code blocks within a conversation.
            These settings control when idle sessions are cleaned up.
          </p>
          <div class="settings-form">
            <label>
              <span>Idle timeout (seconds)</span>
              <input
                type="number"
                min="60"
                value={ca().idle_timeout_secs}
                onChange={(e) => update('idle_timeout_secs', Math.max(60, parseInt(e.currentTarget.value) || 600))}
              />
            </label>
            <label>
              <span>Max concurrent sessions</span>
              <input
                type="number"
                min="1"
                max="20"
                value={ca().max_sessions}
                onChange={(e) => update('max_sessions', Math.max(1, parseInt(e.currentTarget.value) || 3))}
              />
            </label>
          </div>

          <h3 style="margin-top: 1.5rem;">Sandbox Permissions</h3>
          <p class="muted" style="font-size:12px;margin-bottom:8px;">
            Control what the executed code is allowed to access.
          </p>
          <div class="settings-form">
            <Switch checked={ca().allow_network} onChange={(checked) => update('allow_network', checked)} class="flex items-center gap-2">
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>Allow network access from executed code</SwitchLabel>
            </Switch>
          </div>
        </>
      </Show>
    </div>
  );
};

export default CodeActTab;
