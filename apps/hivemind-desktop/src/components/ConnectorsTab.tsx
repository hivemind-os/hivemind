import { For, Show, createSignal, onMount, onCleanup } from 'solid-js';
import { Plug, Link } from 'lucide-solid';
import { invoke } from '@tauri-apps/api/core';
import { openExternal } from '../utils';
import {
  type ConnectorConfig,
  type ConnectorProvider,
  type ConnectorStatus,
  type DiscoveredChannel,
  type DiscoveredGuild,
  type DiscoveryState,
  type InstalledPlugin,
  type Integration,
  type OAuthState,
  type OAuthStatus,
  type ServiceType,
  connectorToIntegration,
  enabledServiceList,
  pluginToIntegration,
  providerCard,
} from './connectors/types';
import { ServiceBadges } from './connectors/shared';
import { ConnectorWizard } from './connectors/ConnectorWizard';
import { ConnectorEditDialog } from './connectors/ConnectorEditDialog';
import { TestConnectionModal } from './connectors/TestConnectionModal';
import { PluginConnectorDialog } from './connectors/PluginConnectorDialog';
import { Button, Badge } from '~/ui';

// ── Component ────────────────────────────────────────────────────

export default function ConnectorsTab(_props: { daemon_url?: string; onConnectorsChanged?: () => void }) {
  // Core state
  const [connectors, setConnectors] = createSignal<ConnectorConfig[]>([]);
  const [plugins, setPlugins] = createSignal<InstalledPlugin[]>([]);
  const [statuses, setStatuses] = createSignal<Record<string, ConnectorStatus>>({});
  const [error, setError] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [testResult, setTestResult] = createSignal<string | null>(null);
  const [testing, setTesting] = createSignal(false);

  // Wizard / edit visibility
  const [showWizard, setShowWizard] = createSignal(false);
  const [editConnector, setEditConnector] = createSignal<ConnectorConfig | null>(null);
  const [editPlugin, setEditPlugin] = createSignal<InstalledPlugin | null>(null);

  // Unified integration list
  const integrations = (): Integration[] => {
    const builtins = connectors().map((c) => connectorToIntegration(c, statuses()[c.id]));
    const pluginIntegrations = plugins()
      .filter((p) => p.plugin_type === 'connector')
      .map(pluginToIntegration);
    return [...builtins, ...pluginIntegrations];
  };

  // OAuth state (shared singleton — only one flow active at a time)
  const [oauthStatus, setOauthStatus] = createSignal<OAuthStatus>('idle');
  const [oauthUserCode, setOauthUserCode] = createSignal<string | null>(null);
  const [oauthVerifyUrl, setOauthVerifyUrl] = createSignal<string | null>(null);
  const [oauthError, setOauthError] = createSignal<string | null>(null);
  let oauthPollTimer: ReturnType<typeof setInterval> | null = null;

  // Discovery state (Discord/Slack)
  const [discovering, setDiscovering] = createSignal(false);
  const [discoveredChannels, setDiscoveredChannels] = createSignal<DiscoveredChannel[]>([]);
  const [discoveredGuilds, setDiscoveredGuilds] = createSignal<DiscoveredGuild[]>([]);
  const [discoveredWorkspace, setDiscoveredWorkspace] = createSignal<string | null>(null);
  const [discoverError, setDiscoverError] = createSignal<string | null>(null);

  onMount(() => { loadConnectors(); loadPlugins(); });
  onCleanup(() => { if (oauthPollTimer) clearInterval(oauthPollTimer); });

  // ── API ──────────────────────────────────────────────────────

  async function loadConnectors() {
    try {
      const data = await invoke<ConnectorConfig[]>('list_connectors');
      setConnectors(Array.isArray(data) ? data : []);
    } catch (e) {
      console.error('Failed to load connectors:', e);
    }
  }

  async function loadPlugins() {
    try {
      const data = await invoke<InstalledPlugin[]>('plugin_list');
      setPlugins(Array.isArray(data) ? data : []);
    } catch (e) {
      console.error('Failed to load plugins:', e);
    }
  }

  async function reloadAll() {
    await Promise.all([loadConnectors(), loadPlugins()]);
  }

  async function saveConnectorsApi(configs: ConnectorConfig[]) {
    setSaving(true);
    setError(null);
    try {
      await invoke('save_connectors', { configs });
      setConnectors(configs);
      _props.onConnectorsChanged?.();
    } catch (e: any) {
      setError(e.message || String(e));
    } finally {
      setSaving(false);
    }
  }

  async function testConnectorFn(id: string, inlineConfig?: ConnectorConfig) {
    setTestResult(null);
    setTesting(true);
    try {
      if (inlineConfig?.provider === 'apple') {
        const services = enabledServiceList(inlineConfig);
        const wantCalendar = services.includes('calendar');
        const wantContacts = services.includes('contacts');
        if (wantCalendar || wantContacts) {
          try {
            const result = await invoke<{ status: string; detail: string }>('request_apple_access', {
              calendar: wantCalendar,
              contacts: wantContacts,
            });
            if (result.status === 'denied') {
              const pane = wantCalendar
                ? 'x-apple.systempreferences:com.apple.preference.security?Privacy_Calendars'
                : 'x-apple.systempreferences:com.apple.preference.security?Privacy_Contacts';
              openExternal(pane);
              setTestResult(
                'Error: Calendar/Contacts access denied. System Settings has been opened — ' +
                'please grant access to hive-daemon, then try again.',
              );
              setTesting(false);
              return;
            }
          } catch (_) {
            // Best-effort; the test_connection will give a clear error if access is missing.
          }
        }
      }

      const data = await invoke<{ error?: string; status?: string }>('test_connector', {
        connector_id: id,
        config: inlineConfig ?? null,
      });
      setTestResult(data.error ? `Error: ${data.error}` : (data.status || 'OK'));
    } catch (e: any) {
      setTestResult(`Error: ${e.message || String(e)}`);
    } finally {
      setTesting(false);
    }
  }

  async function deleteConnector(id: string) {
    const updated = connectors().filter((c) => c.id !== id);
    await saveConnectorsApi(updated);
  }

  // ── OAuth ────────────────────────────────────────────────────

  function resetOauthState() {
    setOauthStatus('idle');
    setOauthUserCode(null);
    setOauthVerifyUrl(null);
    setOauthError(null);
    if (oauthPollTimer) { clearInterval(oauthPollTimer); oauthPollTimer = null; }
  }

  async function startOauthFlow(connectorId: string, provider: ConnectorProvider, email?: string, services?: ServiceType[], clientId?: string, clientSecret?: string) {
    resetOauthState();
    setOauthStatus('waiting');
    try {
      let data: any;
      try {
        data = await invoke('connector_oauth_start', {
          connector_id: connectorId,
          provider,
          email: email ?? null,
          services: services ?? null,
          client_id: clientId ?? null,
          client_secret: clientSecret ?? null,
        });
      } catch (e: any) {
        setOauthStatus('error');
        setOauthError(e.message || String(e));
        return;
      }

      if (data.error) {
        setOauthStatus('error');
        if (data.error === 'missing_credentials') {
          setOauthError(`${data.message}\n\nSetup: ${data.setup_hint}\n\nThen restart with ${data.env_var} set.`);
        } else {
          setOauthError(data.message || data.error || 'Failed to start OAuth');
        }
        return;
      }

      const flow = data.flow || 'device_code';
      if (flow === 'browser') {
        openExternal(data.auth_url);
        setOauthStatus('polling');
        setOauthVerifyUrl(data.auth_url);
        let browserPolling = false;
        oauthPollTimer = setInterval(async () => {
          if (browserPolling) return;
          browserPolling = true;
          try {
            const pollData = await invoke<any>('connector_oauth_poll', {
              connector_id: connectorId,
              flow: 'browser',
              device_code: null,
            });
            if (pollData.status === 'complete') {
              setOauthStatus('complete');
              if (oauthPollTimer) { clearInterval(oauthPollTimer); oauthPollTimer = null; }
              await loadConnectors();
              _props.onConnectorsChanged?.();
            } else if (pollData.status === 'failed') {
              setOauthStatus('error');
              setOauthError(pollData.error || 'OAuth authorization failed');
              if (oauthPollTimer) { clearInterval(oauthPollTimer); oauthPollTimer = null; }
            }
          } catch { /* keep polling */ } finally { browserPolling = false; }
        }, 5000);
      } else {
        setOauthUserCode(data.user_code);
        setOauthVerifyUrl(data.verification_uri);
        const deviceCode = data.device_code;
        const interval = (data.interval || 5) * 1000;
        setOauthStatus('polling');
        let devicePolling = false;
        oauthPollTimer = setInterval(async () => {
          if (devicePolling) return;
          devicePolling = true;
          try {
            const pollData = await invoke<any>('connector_oauth_poll', {
              connector_id: connectorId,
              flow: 'device_code',
              device_code: deviceCode,
            });
            if (pollData.status === 'complete') {
              setOauthStatus('complete');
              if (oauthPollTimer) { clearInterval(oauthPollTimer); oauthPollTimer = null; }
              await loadConnectors();
              _props.onConnectorsChanged?.();
            } else if (pollData.status === 'failed') {
              setOauthStatus('error');
              setOauthError(pollData.error || 'OAuth authorization failed');
              if (oauthPollTimer) { clearInterval(oauthPollTimer); oauthPollTimer = null; }
            }
          } catch { /* keep polling */ } finally { devicePolling = false; }
        }, interval);
      }
    } catch (e: any) {
      setOauthStatus('error');
      setOauthError(e.message || 'Failed to start OAuth flow');
    }
  }

  // ── Discovery (Discord/Slack) ────────────────────────────────

  async function discoverChannels(connectorId: string, provider: ConnectorProvider, botToken: string) {
    setDiscovering(true);
    setDiscoveredChannels([]);
    setDiscoveredGuilds([]);
    setDiscoveredWorkspace(null);
    setDiscoverError(null);
    try {
      const data = await invoke<any>('connector_discover', {
        connector_id: connectorId,
        provider_type: provider,
        bot_token: botToken,
      });
      if (data.error) { setDiscoverError(data.error); return; }
      if (data.guilds) setDiscoveredGuilds(data.guilds);
      if (data.channels) setDiscoveredChannels(data.channels);
      if (data.workspace_name) setDiscoveredWorkspace(data.workspace_name);
    } catch (e: any) {
      setDiscoverError(e.message || String(e));
    } finally {
      setDiscovering(false);
    }
  }

  // ── State bundles for sub-components ─────────────────────────

  const oauthState: OAuthState = {
    status: oauthStatus,
    userCode: oauthUserCode,
    verifyUrl: oauthVerifyUrl,
    error: oauthError,
    start: startOauthFlow,
    reset: resetOauthState,
  };

  const discoveryState: DiscoveryState = {
    discovering,
    channels: discoveredChannels,
    guilds: discoveredGuilds,
    workspace: discoveredWorkspace,
    error: discoverError,
    discover: discoverChannels,
  };

  // ── Wizard / Edit lifecycle ──────────────────────────────────

  function openWizard() {
    resetOauthState();
    setTestResult(null);
    setShowWizard(true);
  }

  function closeWizard() {
    setShowWizard(false);
    resetOauthState();
  }

  async function finishWizard(config: ConnectorConfig) {
    const updated = [...connectors(), config];
    await saveConnectorsApi(updated);
    if (!error()) closeWizard();
  }

  function openEdit(id: string) {
    const c = connectors().find((x) => x.id === id);
    if (!c) return;
    setEditConnector(JSON.parse(JSON.stringify(c)));
    setTestResult(null);
    resetOauthState();
  }

  function closeEdit() {
    setEditConnector(null);
    setTestResult(null);
  }

  async function saveEdit(config: ConnectorConfig) {
    const updated = connectors().map((c) => (c.id === config.id ? config : c));
    await saveConnectorsApi(updated);
    if (!error()) closeEdit();
  }

  // ── Main render ──────────────────────────────────────────────

  const statusColorClass = (state?: string) => {
    switch (state) {
      case 'connected': return 'bg-green-500/20 text-green-400';
      case 'error': return 'bg-red-500/20 text-red-400';
      case 'auth-expired': return 'bg-yellow-500/20 text-yellow-400';
      default: return 'bg-secondary text-muted-foreground';
    }
  };

  return (
    <section class="settings-section">
      {/* Error banner */}
      <Show when={error()}>
        <div class="mb-4 flex items-center justify-between rounded-xl border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300">
          <span>{error()}</span>
          <button
            onClick={() => setError(null)}
            class="w-auto cursor-pointer border-none bg-transparent p-0 text-red-300"
          >✕</button>
        </div>
      </Show>

      {/* Integration list (built-in connectors + plugin connectors) */}
      <Show when={integrations().length > 0} fallback={
        <div class="py-12 text-center text-muted-foreground">
          <div class="mb-3 text-4xl"><Plug size={32} /></div>
          <p class="mb-1 text-base font-medium text-foreground/70">No connectors configured yet</p>
          <p class="mb-5 text-sm">Add a connector to link your email, calendar, or chat accounts.</p>
          <Button onClick={openWizard}>+ Add Connector</Button>
        </div>
      }>
        <div class="flex flex-col gap-3">
          <For each={integrations()}>
            {(item) => (
              <article class="rounded-lg border border-input bg-card p-3">
                <header class="flex items-center justify-between">
                  <div class="flex flex-1 items-center gap-2.5">
                    <span class="text-xl">{item.icon}</span>
                    <div>
                      <div class="text-sm font-semibold text-foreground">
                        {item.name}
                      </div>
                      <div class="text-xs text-muted-foreground">
                        {item.kind === 'builtin'
                          ? `${providerCard(item.connector!.provider)?.title ?? ''} · ${item.connector!.auth.type}`
                          : `${item.plugin!.name} · v${item.plugin!.version}`
                        }
                      </div>
                    </div>
                  </div>
                  <div class="flex items-center gap-2">
                    <Show when={item.kind === 'builtin' && item.connector}>
                      <ServiceBadges config={item.connector!} />
                    </Show>
                    <Show when={item.kind === 'plugin'}>
                      <Badge variant="outline" class="text-[0.65rem]">plugin</Badge>
                    </Show>
                    <Show when={item.status}>
                      <Badge variant="secondary" class={statusColorClass(item.status?.state)}>
                        {item.status?.state}
                      </Badge>
                    </Show>
                    <Show when={!item.enabled}>
                      <Badge variant="outline" class="text-[0.72rem]">disabled</Badge>
                    </Show>
                  </div>
                </header>
                <div class="mt-2 flex justify-end gap-2">
                  <Show when={item.kind === 'builtin'}>
                    <Button variant="secondary" size="sm" onClick={() => openEdit(item.connector!.id)}>Edit</Button>
                    <Button variant="secondary" size="sm" onClick={() => testConnectorFn(item.connector!.id)}>Test</Button>
                    <Button variant="destructive" size="sm" onClick={() => deleteConnector(item.connector!.id)}>Delete</Button>
                  </Show>
                  <Show when={item.kind === 'plugin'}>
                    <Button variant="secondary" size="sm" onClick={() => setEditPlugin(item.plugin!)}>Configure</Button>
                  </Show>
                </div>
              </article>
            )}
          </For>
        </div>
        <div class="mt-4">
          <Button onClick={openWizard}>+ Add Connector</Button>
        </div>
      </Show>

      {/* Wizard Modal */}
      <Show when={showWizard()}>
        <ConnectorWizard
          existingIds={connectors().map((c) => c.id)}
          oauth={oauthState}
          discovery={discoveryState}
          testing={testing}
          testResult={testResult}
          saving={saving}
          onTest={testConnectorFn}
          onFinish={finishWizard}
          onClose={closeWizard}
        />
      </Show>

      {/* Edit Modal */}
      <Show when={editConnector()}>
        {(conn) => (
          <ConnectorEditDialog
            connector={conn()}
            oauth={oauthState}
            saving={saving}
            testing={testing}
            testResult={testResult}
            onTest={(id) => testConnectorFn(id)}
            onSave={saveEdit}
            onClose={closeEdit}
          />
        )}
      </Show>

      {/* Plugin Connector Dialog */}
      <Show when={editPlugin()}>
        {(plugin) => (
          <PluginConnectorDialog
            plugin={plugin()}
            onClose={() => setEditPlugin(null)}
            onSave={() => { setEditPlugin(null); reloadAll(); }}
          />
        )}
      </Show>

      {/* Test Connection Modal */}
      <TestConnectionModal
        testing={testing}
        testResult={testResult}
        onClose={() => setTestResult(null)}
      />
    </section>
  );
}