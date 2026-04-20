import { createSignal, Show, type Component } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import PluginConfigForm from '../plugins/PluginConfigForm';
import type { PluginConfigSchema } from '../plugins/PluginConfigForm';
import type { InstalledPlugin } from './types';
import { Button, Badge } from '~/ui';

export interface PluginConnectorDialogProps {
  plugin: InstalledPlugin;
  onClose: () => void;
  onSave: () => void;
}

export function PluginConnectorDialog(props: PluginConnectorDialogProps) {
  const [editConfig, setEditConfig] = createSignal<Record<string, any>>(
    { ...props.plugin.config }
  );
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const schema = () => props.plugin.config_schema;

  async function saveConfig() {
    setSaving(true);
    setError(null);
    try {
      await invoke('plugin_save_config', {
        plugin_id: props.plugin.plugin_id,
        config: editConfig(),
      });
      props.onSave();
    } catch (e: any) {
      setError(e?.message || String(e));
    } finally {
      setSaving(false);
    }
  }

  async function toggleEnabled() {
    try {
      await invoke('plugin_set_enabled', {
        plugin_id: props.plugin.plugin_id,
        enabled: !props.plugin.enabled,
      });
      props.onSave();
    } catch (e: any) {
      setError(e?.message || String(e));
    }
  }

  async function uninstall() {
    try {
      await invoke('plugin_uninstall', {
        plugin_id: props.plugin.plugin_id,
      });
      props.onSave();
      props.onClose();
    } catch (e: any) {
      setError(e?.message || String(e));
    }
  }

  return (
    <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={props.onClose}>
      <div
        class="flex max-h-[85vh] w-full max-w-lg flex-col overflow-hidden rounded-2xl border border-input bg-popover shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <header class="flex items-start justify-between border-b border-input px-6 py-4">
          <div class="flex items-center gap-3">
            <span class="text-2xl">🧩</span>
            <div>
              <h2 class="text-base font-semibold text-foreground">
                {props.plugin.display_name || props.plugin.name}
              </h2>
              <p class="text-xs text-muted-foreground">
                {props.plugin.name} v{props.plugin.version}
              </p>
            </div>
          </div>
          <button onClick={props.onClose} class="cursor-pointer border-none bg-transparent p-1 text-muted-foreground hover:text-foreground">✕</button>
        </header>

        {/* Body */}
        <div class="flex-1 overflow-y-auto px-6 py-4">
          {/* Description */}
          <Show when={props.plugin.description}>
            <p class="mb-4 text-sm text-muted-foreground">{props.plugin.description}</p>
          </Show>

          {/* Status */}
          <Show when={props.plugin.status}>
            <div class="mb-4 flex items-center gap-2">
              <span class="text-xs text-muted-foreground">Status:</span>
              <Badge variant="secondary" class={
                props.plugin.status?.state === 'connected' ? 'bg-green-500/20 text-green-400' :
                props.plugin.status?.state === 'error' ? 'bg-red-500/20 text-red-400' :
                'bg-secondary text-muted-foreground'
              }>
                {props.plugin.status?.state}
              </Badge>
              <Show when={props.plugin.status?.message}>
                <span class="text-xs text-muted-foreground">{props.plugin.status?.message}</span>
              </Show>
            </div>
          </Show>

          {/* Permissions */}
          <Show when={props.plugin.permissions.length > 0}>
            <div class="mb-4">
              <span class="mb-1 block text-xs font-medium text-muted-foreground">Permissions</span>
              <div class="flex flex-wrap gap-1">
                {props.plugin.permissions.map((p) => (
                  <Badge variant="outline" class="text-[0.65rem]">{p}</Badge>
                ))}
              </div>
            </div>
          </Show>

          {/* Error */}
          <Show when={error()}>
            <div class="mb-3 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-300">
              {error()}
            </div>
          </Show>

          {/* Config Form */}
          <Show when={schema()} fallback={
            <p class="text-sm text-muted-foreground italic">No configuration options available.</p>
          }>
            {(s) => (
              <PluginConfigForm
                schema={s()}
                values={editConfig()}
                onChange={(key, value) => {
                  setEditConfig((prev) => ({ ...prev, [key]: value }));
                }}
              />
            )}
          </Show>
        </div>

        {/* Footer */}
        <footer class="flex items-center justify-between border-t border-input px-6 py-3">
          <div class="flex gap-2">
            <Button
              variant={props.plugin.enabled ? 'secondary' : 'default'}
              size="sm"
              onClick={toggleEnabled}
            >
              {props.plugin.enabled ? 'Disable' : 'Enable'}
            </Button>
            <Button variant="destructive" size="sm" onClick={uninstall}>
              Uninstall
            </Button>
          </div>
          <div class="flex gap-2">
            <Button variant="secondary" size="sm" onClick={props.onClose}>Cancel</Button>
            <Show when={schema()}>
              <Button size="sm" onClick={saveConfig} disabled={saving()}>
                {saving() ? 'Saving…' : 'Save'}
              </Button>
            </Show>
          </div>
        </footer>
      </div>
    </div>
  );
}
