import { For, Show, createSignal, createResource, onMount } from 'solid-js';
import { Link, Search, TriangleAlert, LoaderCircle, Download, CheckCircle } from 'lucide-solid';
import { invoke } from '@tauri-apps/api/core';
import { openExternal } from '../../utils';
import { PersonaSelector, type PersonaInfo } from '../shared';
import {
  type AuthConfig,
  type CalendarConfig,
  type CommunicationConfig,
  type ConnectorConfig,
  type ConnectorProvider,
  type ContactsConfig,
  type DiscoveryState,
  type DriveConfig,
  type OAuthState,
  type ResourceRule,
  type ServiceType,
  type TradingConfig,
  type WizardStep,
  PROVIDER_CARDS,
  SERVICE_INFO,
  STEP_LABELS,
  createEmptyConnector,
  defaultCalendarConfig,
  defaultCommConfig,
  defaultContactsConfig,
  defaultDriveConfig,
  defaultTradingConfig,
  enabledServiceList,
  getDiscordInviteUrl,
  isSingleServiceProvider,
  providerCard,
  providerServices,
} from './types';
import { ClassificationSelect, CommConfigForm, OAuthFlow, ServiceAccordion } from './shared';
import { Dialog, DialogContent } from '~/ui/dialog';
import { Button } from '~/ui/button';

// ── Props ────────────────────────────────────────────────────────

interface ConnectorWizardProps {
  existingIds: string[];
  oauth: OAuthState;
  discovery: DiscoveryState;
  testing: () => boolean;
  testResult: () => string | null;
  saving: () => boolean;
  onTest: (id: string, inlineConfig: ConnectorConfig) => Promise<void>;
  onFinish: (config: ConnectorConfig) => Promise<void>;
  onClose: () => void;
}

// ── Plugin Install Section (for ConnectorWizard provider step) ──

const REGISTRY_URL = 'https://raw.githubusercontent.com/hivemind-os/hivemind/main/packages/plugin-registry/registry.json';

interface RegistryPlugin {
  name: string;
  displayName: string;
  description: string;
  npmPackage: string;
  categories: string[];
  verified: boolean;
  featured: boolean;
  icon?: string;
  author: string;
  pluginType: string;
}

function PluginInstallSection(props: { onInstalled: () => void }) {
  const [registry] = createResource(async () => {
    try {
      const res = await fetch(REGISTRY_URL);
      if (!res.ok) return null;
      return (await res.json()) as { plugins: RegistryPlugin[] };
    } catch {
      return null;
    }
  });
  const [search, setSearch] = createSignal('');
  const [installing, setInstalling] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [installed, setInstalled] = createSignal<Set<string>>(new Set());

  const filtered = () => {
    const data = registry();
    if (!data) return [];
    const q = search().toLowerCase().trim();
    let list = data.plugins;
    if (q) {
      list = list.filter(
        (p) =>
          p.displayName.toLowerCase().includes(q) ||
          p.description.toLowerCase().includes(q) ||
          p.name.toLowerCase().includes(q)
      );
    }
    return list.sort((a, b) => {
      if (a.featured !== b.featured) return a.featured ? -1 : 1;
      return a.displayName.localeCompare(b.displayName);
    });
  };

  async function installPlugin(npmPackage: string) {
    setInstalling(npmPackage);
    setError(null);
    try {
      await invoke('plugin_install_npm', { packageName: npmPackage });
      setInstalled((prev) => new Set([...prev, npmPackage]));
      props.onInstalled();
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setInstalling(null);
    }
  }

  async function linkLocal() {
    setError(null);
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const folder = await open({ directory: true, multiple: false, title: 'Select plugin folder (must contain package.json)' });
      if (!folder) return;
      const path = typeof folder === 'string' ? folder : (folder as any)[0];
      if (!path) return;
      setInstalling('__local__');
      await invoke('plugin_link_local', { path });
      props.onInstalled();
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setInstalling(null);
    }
  }

  return (
    <div>
      <Show when={error()}>
        <div style={{ background: 'hsl(var(--destructive) / 0.1)', border: '1px solid hsl(var(--destructive) / 0.3)', 'border-radius': '0.5rem', padding: '0.5rem', 'font-size': '0.78rem', color: 'hsl(var(--destructive))', 'margin-bottom': '0.5rem' }}>
          {error()}
        </div>
      </Show>

      {/* Link local plugin — prominent for developers */}
      <button
        onClick={linkLocal}
        disabled={installing() === '__local__'}
        style={{
          display: 'flex', 'align-items': 'center', gap: '0.5rem', width: '100%',
          padding: '0.65rem 0.85rem', 'margin-bottom': '0.75rem',
          background: 'hsl(var(--card) / 0.5)',
          border: '1px dashed hsl(var(--primary) / 0.4)', 'border-radius': '0.6rem',
          cursor: 'pointer', 'font-size': '0.82rem', color: 'hsl(var(--foreground))',
        }}
      >
        <span style={{ 'font-size': '1.1rem' }}>📂</span>
        <div style={{ flex: '1', 'text-align': 'left' }}>
          <div style={{ 'font-weight': '600' }}>Link Local Plugin</div>
          <div style={{ 'font-size': '0.73rem', color: 'hsl(var(--muted-foreground))' }}>
            Select a folder containing a built plugin (for development &amp; testing)
          </div>
        </div>
        <Show when={installing() === '__local__'}>
          <LoaderCircle size={14} style={{ animation: 'spin 1s linear infinite' }} />
        </Show>
      </button>

      <div style={{ position: 'relative', 'margin-bottom': '0.75rem' }}>
        <Search size={14} style={{ position: 'absolute', left: '0.65rem', top: '50%', transform: 'translateY(-50%)', color: 'hsl(var(--muted-foreground))' }} />
        <input
          type="text"
          value={search()}
          onInput={(e) => setSearch(e.currentTarget.value)}
          placeholder="Search community plugins..."
          style={{
            width: '100%', 'padding-left': '2rem', 'padding-right': '0.75rem',
            'padding-top': '0.4rem', 'padding-bottom': '0.4rem',
            'border-radius': '0.5rem', border: '1px solid hsl(var(--border) / 0.3)',
            background: 'hsl(var(--background))', 'font-size': '0.82rem',
          }}
        />
      </div>

      <Show when={registry.loading}>
        <div style={{ 'text-align': 'center', padding: '1rem', color: 'hsl(var(--muted-foreground))', 'font-size': '0.82rem' }}>
          <LoaderCircle size={16} style={{ animation: 'spin 1s linear infinite', display: 'inline-block' }} /> Loading registry...
        </div>
      </Show>

      <Show when={!registry.loading && filtered().length === 0}>
        <div style={{ 'text-align': 'center', padding: '1rem', color: 'hsl(var(--muted-foreground))', 'font-size': '0.82rem' }}>
          {registry() ? 'No plugins match your search.' : 'Could not load plugin registry.'}
        </div>
      </Show>

      <div style={{ display: 'flex', 'flex-direction': 'column', gap: '0.5rem', 'max-height': '16rem', 'overflow-y': 'auto' }}>
        <For each={filtered()}>
          {(plugin) => {
            const isInstalled = () => installed().has(plugin.npmPackage);
            const isInstalling = () => installing() === plugin.npmPackage;
            return (
              <div style={{
                display: 'flex', 'align-items': 'center', gap: '0.75rem',
                padding: '0.65rem 0.85rem', background: 'hsl(var(--card) / 0.5)',
                border: '1px solid hsl(var(--border) / 0.12)', 'border-radius': '0.6rem',
              }}>
                <div style={{ 'font-size': '1.3rem', 'flex-shrink': '0' }}>{plugin.icon || '🧩'}</div>
                <div style={{ flex: '1', 'min-width': '0' }}>
                  <div style={{ 'font-size': '0.85rem', 'font-weight': '600', color: 'hsl(var(--foreground))' }}>
                    {plugin.displayName}
                    <Show when={plugin.verified}>
                      <span style={{ color: 'hsl(var(--primary))', 'margin-left': '0.3rem', 'font-size': '0.75rem' }}>✓</span>
                    </Show>
                  </div>
                  <div style={{ 'font-size': '0.73rem', color: 'hsl(var(--muted-foreground))', overflow: 'hidden', 'text-overflow': 'ellipsis', 'white-space': 'nowrap' }}>
                    {plugin.description}
                  </div>
                </div>
                <Show when={isInstalled()}>
                  <CheckCircle size={16} style={{ color: 'hsl(var(--primary))', 'flex-shrink': '0' }} />
                </Show>
                <Show when={!isInstalled()}>
                  <button
                    onClick={() => installPlugin(plugin.npmPackage)}
                    disabled={isInstalling()}
                    style={{
                      display: 'flex', 'align-items': 'center', gap: '0.3rem',
                      padding: '0.3rem 0.6rem', 'border-radius': '0.4rem',
                      background: 'hsl(var(--primary))', color: 'hsl(var(--primary-foreground))',
                      border: 'none', cursor: 'pointer', 'font-size': '0.75rem', 'font-weight': '600',
                      opacity: isInstalling() ? '0.7' : '1', 'flex-shrink': '0',
                    }}
                  >
                    {isInstalling() ? (
                      <LoaderCircle size={12} style={{ animation: 'spin 1s linear infinite' }} />
                    ) : (
                      <Download size={12} />
                    )}
                    {isInstalling() ? 'Installing...' : 'Install'}
                  </button>
                </Show>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}

// ── Component ────────────────────────────────────────────────────

export function ConnectorWizard(props: ConnectorWizardProps) {
  const [draft, setDraft] = createSignal<ConnectorConfig>(createEmptyConnector('microsoft', props.existingIds));
  const [selectedServices, setSelectedServices] = createSignal<Set<ServiceType>>(new Set(providerServices('microsoft')));
  const [wizardStep, setWizardStep] = createSignal<WizardStep>('provider');

  // Persona access control
  const [availablePersonas, setAvailablePersonas] = createSignal<{ id: string; name: string }[]>([]);
  const [selectedPersonas, setSelectedPersonas] = createSignal<string[]>([]);

  onMount(() => {
    invoke<{ id: string; name: string }[]>('list_personas', { include_archived: false })
      .then((ps) => setAvailablePersonas(ps.map((p) => ({ id: p.id, name: p.name }))))
      .catch(() => {});
  });

  // ── Updater helpers ──────────────────────────────────────────

  function updateDraft(fn: (d: ConnectorConfig) => ConnectorConfig) {
    setDraft(fn(draft()));
  }

  function updateAuth<K extends keyof AuthConfig>(key: K, value: AuthConfig[K]) {
    updateDraft((d) => ({ ...d, auth: { ...d.auth, [key]: value } }));
  }

  function updateCommConfig<K extends keyof CommunicationConfig>(key: K, value: CommunicationConfig[K]) {
    updateDraft((d) => {
      const comm = d.services.communication;
      if (!comm) return d;
      return { ...d, services: { ...d.services, communication: { ...comm, [key]: value } } };
    });
  }

  function updateCalendarConfig<K extends keyof CalendarConfig>(key: K, value: CalendarConfig[K]) {
    updateDraft((d) => {
      const cal = d.services.calendar;
      if (!cal) return d;
      return { ...d, services: { ...d.services, calendar: { ...cal, [key]: value } } };
    });
  }

  function updateDriveConfig<K extends keyof DriveConfig>(key: K, value: DriveConfig[K]) {
    updateDraft((d) => {
      const drv = d.services.drive;
      if (!drv) return d;
      return { ...d, services: { ...d.services, drive: { ...drv, [key]: value } } };
    });
  }

  function updateContactsConfig<K extends keyof ContactsConfig>(key: K, value: ContactsConfig[K]) {
    updateDraft((d) => {
      const ctn = d.services.contacts;
      if (!ctn) return d;
      return { ...d, services: { ...d.services, contacts: { ...ctn, [key]: value } } };
    });
  }

  function updateTradingConfig<K extends keyof TradingConfig>(key: K, value: TradingConfig[K]) {
    updateDraft((d) => {
      const trd = d.services.trading;
      if (!trd) return d;
      return { ...d, services: { ...d.services, trading: { ...trd, [key]: value } } };
    });
  }

  function addDestinationRule() {
    updateDraft((d) => {
      const comm = d.services.communication;
      if (!comm) return d;
      return { ...d, services: { ...d.services, communication: { ...comm, destination_rules: [...comm.destination_rules, { pattern: '', approval: 'ask' as const }] } } };
    });
  }

  function removeDestinationRule(idx: number) {
    updateDraft((d) => {
      const comm = d.services.communication;
      if (!comm) return d;
      return { ...d, services: { ...d.services, communication: { ...comm, destination_rules: comm.destination_rules.filter((_, i) => i !== idx) } } };
    });
  }

  function updateDestinationRule(idx: number, key: keyof ResourceRule, value: any) {
    updateDraft((d) => {
      const comm = d.services.communication;
      if (!comm) return d;
      const rules = comm.destination_rules.map((r, i) => (i === idx ? { ...r, [key]: value } : r));
      return { ...d, services: { ...d.services, communication: { ...comm, destination_rules: rules } } };
    });
  }

  // ── Step navigation ──────────────────────────────────────────

  function wizardSteps(): WizardStep[] {
    if (isSingleServiceProvider(draft().provider)) {
      return ['provider', 'connect', 'configure', 'review'];
    }
    return ['provider', 'services', 'connect', 'configure', 'review'];
  }

  function currentStepIdx(): number {
    return wizardSteps().indexOf(wizardStep());
  }

  function nextWizardStep() {
    const steps = wizardSteps();
    const idx = steps.indexOf(wizardStep());
    if (idx < steps.length - 1) setWizardStep(steps[idx + 1]);
  }

  function prevWizardStep() {
    const steps = wizardSteps();
    const idx = steps.indexOf(wizardStep());
    if (idx > 0) setWizardStep(steps[idx - 1]);
  }

  function canAdvance(): boolean {
    const step = wizardStep();
    const d = draft();

    if (step === 'provider') return true;
    if (step === 'services') return selectedServices().size > 0;

    if (step === 'connect') {
      if (!d.name.trim()) return false;
      const auth = d.auth;
      if (auth.type === 'oauth2') return props.oauth.status() === 'complete';
      if (auth.type === 'bot-token') {
        if (d.provider === 'slack') return !!(auth.bot_token && auth.app_token);
        return !!auth.bot_token;
      }
      if (auth.type === 'password') {
        return !!(auth.username && auth.password && auth.imap_host && auth.smtp_host);
      }
      if (auth.type === 'cdp-api-key') {
        return !!(auth.key_name?.trim() && auth.private_key?.trim());
      }
      if (auth.type === 'local') return true;
      return false;
    }

    return true;
  }

  function selectProvider(p: ConnectorProvider) {
    const c = createEmptyConnector(p, props.existingIds);
    setDraft(c);
    setSelectedServices(new Set(providerServices(p)));
    props.oauth.reset();
    nextWizardStep();
  }

  function toggleService(svc: ServiceType) {
    const curr = new Set(selectedServices());
    if (curr.has(svc)) curr.delete(svc);
    else curr.add(svc);
    setSelectedServices(curr);

    updateDraft((d) => {
      const services = { ...d.services };
      if (svc === 'communication') services.communication = curr.has('communication') ? (services.communication || defaultCommConfig()) : null;
      if (svc === 'calendar') services.calendar = curr.has('calendar') ? (services.calendar || defaultCalendarConfig()) : null;
      if (svc === 'drive') services.drive = curr.has('drive') ? (services.drive || defaultDriveConfig()) : null;
      if (svc === 'contacts') services.contacts = curr.has('contacts') ? (services.contacts || defaultContactsConfig()) : null;
      if (svc === 'trading') services.trading = curr.has('trading') ? (services.trading || defaultTradingConfig()) : null;
      return { ...d, services };
    });
  }

  // ── Step renderers ───────────────────────────────────────────

  const isMacOS = navigator.platform?.startsWith('Mac') ?? false;

  function renderProviderStep() {
    const cards = PROVIDER_CARDS.filter((c) => !c.platform || (c.platform === 'macos' && isMacOS));
    return (
      <div>
        <p style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.88rem', 'margin-bottom': '1rem' }}>
          Choose a connector provider:
        </p>
        <div class="channel-type-list">
          <For each={cards}>
            {(card) => (
              <div
                class="channel-type-row"
                classList={{ disabled: !card.enabled }}
                onClick={() => card.enabled && selectProvider(card.provider)}
              >
                <div class="channel-type-row-icon">{card.icon}</div>
                <div class="channel-type-row-body">
                  <div class="channel-type-row-title">{card.title}</div>
                  <div class="channel-type-row-desc">{card.desc}</div>
                </div>
                <div class="channel-type-row-badges">
                  <For each={card.services}>
                    {(svc) => (
                      <span class="channel-type-card-badge">{SERVICE_INFO[svc].icon} {SERVICE_INFO[svc].label}</span>
                    )}
                  </For>
                </div>
              </div>
            )}
          </For>
        </div>

        {/* Community Plugins section */}
        <div style={{ 'margin-top': '1.5rem', 'border-top': '1px solid hsl(var(--border) / 0.15)', 'padding-top': '1rem' }}>
          <p style={{ 'font-size': '0.82rem', 'font-weight': '600', color: 'hsl(var(--foreground))', 'margin-bottom': '0.5rem' }}>
            🧩 Community Plugins
          </p>
          <p style={{ 'font-size': '0.78rem', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '0.75rem' }}>
            Install third-party connector plugins from the registry. Plugins appear alongside built-in connectors after installation.
          </p>
          <PluginInstallSection onInstalled={() => props.onClose()} />
        </div>
      </div>
    );
  }

  function renderServicesStep() {
    const available = () => providerServices(draft().provider);
    const microsoftScopes: Partial<Record<ServiceType, string>> = {
      communication: 'Mail.ReadWrite, Mail.Send',
      calendar: 'Calendars.ReadWrite',
      drive: 'Files.ReadWrite.All',
      contacts: 'Contacts.Read',
    };
    const gmailScopes: Partial<Record<ServiceType, string>> = {
      communication: 'gmail.modify, gmail.send',
      calendar: 'calendar',
      drive: 'drive',
      contacts: 'contacts.readonly',
    };
    const coinbaseScopes: Partial<Record<ServiceType, string>> = {
      trading: 'wallet:accounts:read, wallet:buys:create, wallet:sells:create, wallet:transactions:read/send',
    };
    const scopeHints: Partial<Record<ServiceType, string>> =
      draft().provider === 'gmail' ? gmailScopes
      : draft().provider === 'microsoft' ? microsoftScopes
      : draft().provider === 'coinbase' ? coinbaseScopes
      : {};

    return (
      <div>
        <p style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.88rem', 'margin-bottom': '1rem' }}>
          Select which services to enable for this connector:
        </p>
        <div style={{ display: 'flex', 'flex-direction': 'column', gap: '0.75rem' }}>
          <For each={available()}>
            {(svc) => {
              const info = SERVICE_INFO[svc];
              const checked = () => selectedServices().has(svc);
              return (
                <label
                  style={{
                    display: 'flex', 'align-items': 'flex-start', gap: '0.75rem',
                    padding: '0.85rem 1rem', background: 'hsl(var(--card) / 0.5)',
                    border: `1px solid ${checked() ? 'hsl(var(--primary) / 0.3)' : 'hsl(var(--border) / 0.1)'}`,
                    'border-radius': '0.75rem', cursor: 'pointer',
                    transition: 'border-color 0.15s ease',
                  }}
                >
                  <input type="checkbox" checked={checked()} onChange={() => toggleService(svc)} style={{ 'margin-top': '0.15rem' }} />
                  <div>
                    <div style={{ 'font-size': '0.92rem', 'font-weight': '600', color: 'hsl(var(--foreground))' }}>
                      {info.icon} {info.label}
                    </div>
                    <div style={{ 'font-size': '0.78rem', color: 'hsl(var(--muted-foreground))', 'margin-top': '0.15rem' }}>{info.desc}</div>
                    <Show when={scopeHints[svc]}>
                      <div style={{ 'font-size': '0.72rem', color: 'hsl(var(--muted-foreground))', 'margin-top': '0.25rem' }}>
                        OAuth scopes: {scopeHints[svc]}
                      </div>
                    </Show>
                  </div>
                </label>
              );
            }}
          </For>
        </div>
      </div>
    );
  }

  function renderConnectStep() {
    return (
      <div>
        <div class="channel-form-group">
          <label class="channel-form-label">Connector Name</label>
          <input
            type="text"
            value={draft().name}
            onInput={(e) => updateDraft((x) => ({ ...x, name: e.currentTarget.value }))}
            placeholder={`My ${providerCard(draft().provider)?.title ?? draft().provider} account`}
          />
          <span class="channel-form-hint">A friendly name for this connector</span>
        </div>

        {/* OAuth providers (Microsoft, Gmail only now) */}
        <Show when={draft().provider === 'microsoft' || draft().provider === 'gmail'}>
          <OAuthFlow connectorId={draft().id} provider={draft().provider} oauth={props.oauth} />
        </Show>

        {/* Coinbase CDP API Key */}
        <Show when={draft().provider === 'coinbase'}>
          <div style={{ 'margin-bottom': '0.75rem' }}>
            <p style={{ 'font-size': '0.82rem', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '0.5rem' }}>
              Enter your Coinbase CDP API key credentials.{' '}
              <a
                href="https://portal.cdp.coinbase.com/"
                onClick={(e) => { e.preventDefault(); openExternal('https://portal.cdp.coinbase.com/'); }}
                style={{ color: 'hsl(var(--primary))', cursor: 'pointer' }}
              >
                Create a key on the Coinbase Developer Platform
              </a>{' '}
              — choose <strong>ECDSA</strong> as the key type.
            </p>
            <div class="channel-form-group" style={{ 'margin-bottom': '0.5rem' }}>
              <label class="channel-form-label">API Key Name</label>
              <input
                type="text"
                value={draft().auth.key_name || ''}
                onInput={(e) => updateAuth('key_name', e.currentTarget.value)}
                placeholder="organizations/{org_id}/apiKeys/{key_id}"
              />
              <span class="channel-form-hint">The full key name from your CDP dashboard</span>
            </div>
            <div class="channel-form-group">
              <label class="channel-form-label">Private Key (PEM)</label>
              <textarea
                value={draft().auth.private_key || ''}
                onInput={(e) => updateAuth('private_key', e.currentTarget.value)}
                placeholder="-----BEGIN EC PRIVATE KEY-----&#10;...&#10;-----END EC PRIVATE KEY-----"
                rows={5}
                style={{ 'font-family': 'monospace', 'font-size': '0.78rem', 'white-space': 'pre', resize: 'vertical' }}
              />
              <span class="channel-form-hint">The EC private key downloaded when you created the API key</span>
            </div>
          </div>
        </Show>

        {/* IMAP credentials */}
        <Show when={draft().provider === 'imap'}>
          <div class="channel-form-row">
            <div class="channel-form-group">
              <label class="channel-form-label">Username / Email</label>
              <input type="text" value={draft().auth.username || ''} onInput={(e) => updateAuth('username', e.currentTarget.value)} placeholder="user@example.com" />
            </div>
            <div class="channel-form-group">
              <label class="channel-form-label">Password</label>
              <input type="password" value={draft().auth.password || ''} onInput={(e) => updateAuth('password', e.currentTarget.value)} placeholder="App password" />
            </div>
          </div>
          <div class="channel-form-row">
            <div class="channel-form-group">
              <label class="channel-form-label">IMAP Host</label>
              <input type="text" value={draft().auth.imap_host || ''} onInput={(e) => updateAuth('imap_host', e.currentTarget.value)} placeholder="imap.example.com" />
            </div>
            <div class="channel-form-group">
              <label class="channel-form-label">IMAP Port</label>
              <input type="number" value={draft().auth.imap_port ?? 993} onInput={(e) => updateAuth('imap_port', parseInt(e.currentTarget.value) || 993)} />
            </div>
          </div>
          <div class="channel-form-row">
            <div class="channel-form-group">
              <label class="channel-form-label">SMTP Host</label>
              <input type="text" value={draft().auth.smtp_host || ''} onInput={(e) => updateAuth('smtp_host', e.currentTarget.value)} placeholder="smtp.example.com" />
            </div>
            <div class="channel-form-group">
              <label class="channel-form-label">SMTP Port</label>
              <input type="number" value={draft().auth.smtp_port ?? 587} onInput={(e) => updateAuth('smtp_port', parseInt(e.currentTarget.value) || 587)} />
            </div>
          </div>
          <div class="channel-form-group">
            <label class="channel-form-label">SMTP Encryption</label>
            <select
              value={draft().auth.smtp_encryption || 'starttls'}
              onChange={(e) => {
                const mode = e.currentTarget.value as 'starttls' | 'implicit-tls';
                updateAuth('smtp_encryption', mode);
                if (mode === 'implicit-tls' && (draft().auth.smtp_port === 587 || !draft().auth.smtp_port)) {
                  updateAuth('smtp_port', 465);
                } else if (mode === 'starttls' && draft().auth.smtp_port === 465) {
                  updateAuth('smtp_port', 587);
                }
              }}
            >
              <option value="starttls">STARTTLS (port 587)</option>
              <option value="implicit-tls">Implicit TLS / SMTPS (port 465)</option>
            </select>
          </div>
        </Show>

        {/* Discord setup */}
        <Show when={draft().provider === 'discord'}>
          {renderDiscordSetup()}
        </Show>

        {/* Slack setup */}
        <Show when={draft().provider === 'slack'}>
          {renderSlackSetup()}
        </Show>

        {/* Apple (local) — no credentials needed */}
        <Show when={draft().provider === 'apple'}>
          <div style={{
            background: 'hsl(var(--card) / 0.5)',
            border: '1px solid hsl(var(--border) / 0.15)',
            'border-radius': '0.5rem',
            padding: '0.75rem 1rem',
            'margin-top': '0.75rem',
            'font-size': '0.85rem',
            color: 'hsl(var(--muted-foreground))',
          }}>
            <p style={{ margin: '0 0 0.5rem', 'font-weight': 600, color: 'hsl(var(--foreground))' }}>No credentials required</p>
            <p style={{ margin: 0 }}>
              This connector uses your local macOS Calendar and Contacts data directly.
              When first used, macOS will prompt you to grant access via System Settings → Privacy & Security.
            </p>
          </div>
        </Show>
      </div>
    );
  }

  function renderDiscordSetup() {
    const instructionBoxStyle = {
      background: 'hsl(var(--card) / 0.5)',
      border: '1px solid hsl(var(--border) / 0.15)',
      'border-radius': '0.75rem',
      padding: '1rem 1.25rem',
      'margin-bottom': '1rem',
    };
    const stepTitleStyle = { 'font-weight': '600', color: 'hsl(var(--foreground))', 'margin-bottom': '0.5rem', 'font-size': '0.92rem' };
    const listStyle = { margin: '0', 'padding-left': '1.25rem', color: 'hsl(var(--muted-foreground))', 'font-size': '0.85rem', 'line-height': '1.7' };

    return (
      <>
        <div style={instructionBoxStyle}>
          <div style={stepTitleStyle}>Step 1: Create a Discord Bot</div>
          <ol style={listStyle}>
            <li>
              Go to the{' '}
              <a href="#" onClick={(e) => { e.preventDefault(); openExternal('https://discord.com/developers/applications'); }}
                style={{ color: 'hsl(var(--primary))', cursor: 'pointer', 'text-decoration': 'underline' }}>
                Discord Developer Portal
              </a>
            </li>
            <li>Click "New Application" → name it → go to "Bot" tab</li>
            <li>Click "Reset Token" and copy it below</li>
            <li>Under "Privileged Gateway Intents", enable <strong style={{ color: 'hsl(var(--foreground))' }}>Message Content Intent</strong></li>
          </ol>
        </div>

        <div class="channel-form-group">
          <label class="channel-form-label">Bot Token</label>
          <input
            type="password"
            value={draft().auth.bot_token || ''}
            onInput={(e) => updateAuth('bot_token', e.currentTarget.value)}
            placeholder="Paste your bot token here"
          />
          <span class="channel-form-hint">From the Discord Developer Portal → Bot → Token</span>
        </div>

        <Show when={getDiscordInviteUrl(draft().auth.bot_token)}>
          <div style={instructionBoxStyle}>
            <div style={stepTitleStyle}>Step 2: Invite Bot to Your Server</div>
            <div style={{ display: 'flex', 'align-items': 'center', gap: '0.75rem' }}>
              <button
                style={{ width: 'auto' }}
                onClick={() => { const url = getDiscordInviteUrl(draft().auth.bot_token); if (url) openExternal(url); }}
              >
                <Link size={14} /> Open Invite Link
              </button>
              <span style={{ 'font-size': '0.8rem', color: 'hsl(var(--muted-foreground))' }}>Opens in browser</span>
            </div>
          </div>
        </Show>

        {/* Channel discovery */}
        <Show when={draft().auth.bot_token}>
          {renderChannelDiscovery('discord')}
        </Show>
      </>
    );
  }

  function renderSlackSetup() {
    const instructionBoxStyle = {
      background: 'hsl(var(--card) / 0.5)',
      border: '1px solid hsl(var(--border) / 0.15)',
      'border-radius': '0.75rem',
      padding: '1rem 1.25rem',
      'margin-bottom': '1rem',
    };
    const stepTitleStyle = { 'font-weight': '600', color: 'hsl(var(--foreground))', 'margin-bottom': '0.5rem', 'font-size': '0.92rem' };
    const listStyle = { margin: '0', 'padding-left': '1.25rem', color: 'hsl(var(--muted-foreground))', 'font-size': '0.85rem', 'line-height': '1.7' };

    return (
      <>
        <div style={instructionBoxStyle}>
          <div style={stepTitleStyle}>1. Create a Slack App</div>
          <ol style={listStyle}>
            <li>
              Go to{' '}
              <a href="#" onClick={(e) => { e.preventDefault(); openExternal('https://api.slack.com/apps'); }}
                style={{ color: 'hsl(var(--primary))', cursor: 'pointer', 'text-decoration': 'underline' }}>
                api.slack.com/apps
              </a>
            </li>
            <li>Click "Create New App" → "From scratch"</li>
            <li>Name it and select your workspace</li>
          </ol>
        </div>

        <div style={instructionBoxStyle}>
          <div style={stepTitleStyle}>2. Enable Socket Mode</div>
          <ol style={listStyle}>
            <li>Go to "Socket Mode" in left sidebar → Toggle <strong style={{ color: 'hsl(var(--foreground))' }}>ON</strong></li>
            <li>Name the token (e.g. "hivemind-socket") → Generate</li>
            <li>Copy the <code style={{ color: 'hsl(40 90% 84%)' }}>xapp-...</code> token below</li>
          </ol>
        </div>

        <div class="channel-form-group">
          <label class="channel-form-label">App-Level Token (xapp-…)</label>
          <input
            type="password"
            value={draft().auth.app_token || ''}
            onInput={(e) => updateAuth('app_token', e.currentTarget.value)}
            placeholder="xapp-1-..."
          />
          <span class="channel-form-hint">Required for Socket Mode connections</span>
        </div>

        <div style={instructionBoxStyle}>
          <div style={stepTitleStyle}>3. Set Bot Permissions & Install</div>
          <ol style={listStyle}>
            <li>Go to "OAuth & Permissions" → Scroll to "Scopes"</li>
            <li>
              Add these Bot Token Scopes:
              <ul style={{ 'list-style': 'disc', 'padding-left': '1rem', 'margin-top': '0.25rem' }}>
                <li><code>chat:write</code></li>
                <li><code>app_mentions:read</code></li>
                <li><code>channels:history</code></li>
                <li><code>channels:read</code></li>
                <li><code>im:history</code></li>
                <li><code>im:read</code></li>
              </ul>
            </li>
            <li>Click "Install to Workspace" at the top → Allow</li>
          </ol>
        </div>

        <div class="channel-form-group">
          <label class="channel-form-label">Bot Token (xoxb-…)</label>
          <input
            type="password"
            value={draft().auth.bot_token || ''}
            onInput={(e) => updateAuth('bot_token', e.currentTarget.value)}
            placeholder="xoxb-..."
          />
          <span class="channel-form-hint">Bot User OAuth Token from "OAuth & Permissions"</span>
        </div>

        <div style={instructionBoxStyle}>
          <div style={stepTitleStyle}>4. Enable Events</div>
          <ol style={listStyle}>
            <li>Go to "Event Subscriptions" → Toggle <strong style={{ color: 'hsl(var(--foreground))' }}>ON</strong></li>
            <li>
              Under "Subscribe to bot events", add:
              <ul style={{ 'list-style': 'disc', 'padding-left': '1rem', 'margin-top': '0.25rem' }}>
                <li><code>message.channels</code></li>
                <li><code>message.im</code></li>
                <li><code>app_mention</code></li>
              </ul>
            </li>
          </ol>
        </div>

        {/* Channel discovery */}
        <Show when={draft().auth.bot_token}>
          {renderChannelDiscovery('slack')}
        </Show>
      </>
    );
  }

  function renderChannelDiscovery(provider: 'discord' | 'slack') {
    const isDiscord = provider === 'discord';
    return (
      <>
        <div style={{ display: 'flex', 'align-items': 'center', gap: '0.75rem', 'margin-bottom': '0.5rem' }}>
          <button
            onClick={() => props.discovery.discover(draft().id, draft().provider, draft().auth.bot_token!)}
            disabled={props.discovery.discovering()}
          >
            {props.discovery.discovering() ? <><LoaderCircle size={14} /> Connecting…</> : <><Search size={14} /> Discover Channels</>}
          </button>
        </div>

        <Show when={props.discovery.error()}>
          <div style={{ color: 'hsl(var(--destructive))', 'font-size': '0.85rem', 'margin-bottom': '0.5rem', display: 'flex', 'align-items': 'center', gap: '4px' }}><TriangleAlert size={14} /> {props.discovery.error()}</div>
        </Show>

        {/* Discord: show guilds */}
        <Show when={isDiscord && props.discovery.guilds().length > 0}>
          <div style={{ 'font-size': '0.85rem', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '0.35rem' }}>
            Servers: {props.discovery.guilds().map((g) => g.name).join(', ')}
          </div>
        </Show>

        {/* Slack: show workspace */}
        <Show when={!isDiscord && props.discovery.workspace()}>
          <div style={{ 'font-size': '0.85rem', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '0.35rem' }}>
            Workspace: <strong style={{ color: 'hsl(var(--foreground))' }}>{props.discovery.workspace()}</strong>
          </div>
        </Show>

        <Show when={props.discovery.channels().length > 0}>
          <div class="channel-discover-list" style={{ 'margin-top': '0.5rem', 'max-height': '260px' }}>
            <For each={props.discovery.channels().filter((c) =>
              isDiscord ? (c.type === 0 || c.type === 'text') : (c.is_channel || c.is_im)
            )}>
              {(ch) => {
                const isListening = () => draft().services.communication?.listen_channel_ids.includes(ch.id) ?? false;
                const isDefault = () => draft().services.communication?.default_send_channel_id === ch.id;
                const label = () => isDiscord ? `#${ch.name}` : (ch.is_im ? `DM (${ch.id})` : `#${ch.name}`);
                return (
                  <label class="channel-discover-item">
                    <input
                      type="checkbox"
                      checked={isListening()}
                      onChange={() => {
                        const comm = draft().services.communication;
                        if (!comm) return;
                        const ids = isListening()
                          ? comm.listen_channel_ids.filter((x) => x !== ch.id)
                          : [...comm.listen_channel_ids, ch.id];
                        updateCommConfig('listen_channel_ids', ids);
                      }}
                    />
                    <span>{label()}</span>
                    <Show when={isDiscord && ch.guild_name}>
                      <span class="channel-discover-meta">({ch.guild_name})</span>
                    </Show>
                    <Show when={!isDiscord && ch.type}>
                      <span class="channel-discover-meta">{ch.is_im ? 'DM' : 'channel'}</span>
                    </Show>
                    <Show when={ch.is_member}>
                      <span style={{ color: 'hsl(160 60% 76%)', 'font-size': '0.7rem' }}>• member</span>
                    </Show>
                    <Show when={isListening()}>
                      <button
                        style={{ 'margin-left': 'auto', 'font-size': '0.7rem', padding: '0.15rem 0.5rem', width: 'auto' }}
                        classList={{ primary: isDefault() }}
                        onClick={(e) => { e.preventDefault(); updateCommConfig('default_send_channel_id', ch.id); }}
                      >
                        {isDefault() ? '✓ Default' : 'Set Default'}
                      </button>
                    </Show>
                  </label>
                );
              }}
            </For>
          </div>
        </Show>
      </>
    );
  }

  function renderConfigureStep() {
    return (
      <div>
        <Show when={draft().services.communication?.enabled}>
          <ServiceAccordion service="communication">
            <CommConfigForm
              config={() => draft().services.communication!}
              provider={() => draft().provider}
              onUpdate={(k, v) => updateCommConfig(k as any, v)}
              onAddRule={addDestinationRule}
              onRemoveRule={removeDestinationRule}
              onUpdateRule={updateDestinationRule}
            />
          </ServiceAccordion>
        </Show>
        <Show when={draft().services.calendar?.enabled}>
          <ServiceAccordion service="calendar">
            <ClassificationSelect
              label="Default Classification"
              value={() => draft().services.calendar!.default_class}
              onChange={(v) => updateCalendarConfig('default_class', v)}
              hint="Classification level for calendar data"
            />
          </ServiceAccordion>
        </Show>
        <Show when={draft().services.drive?.enabled}>
          <ServiceAccordion service="drive">
            <ClassificationSelect
              label="Default Classification"
              value={() => draft().services.drive!.default_class}
              onChange={(v) => updateDriveConfig('default_class', v)}
              hint="Classification level for file data"
            />
          </ServiceAccordion>
        </Show>
        <Show when={draft().services.contacts?.enabled}>
          <ServiceAccordion service="contacts">
            <ClassificationSelect
              label="Default Classification"
              value={() => draft().services.contacts!.default_class}
              onChange={(v) => updateContactsConfig('default_class', v)}
              hint="Classification level for contact data"
            />
          </ServiceAccordion>
        </Show>
        <Show when={draft().services.trading?.enabled}>
          <ServiceAccordion service="trading">
            <div class="channel-form-row">
              <ClassificationSelect
                label="Inbound Classification"
                value={() => draft().services.trading!.default_input_class}
                onChange={(v) => updateTradingConfig('default_input_class', v)}
                hint="Classification for data read from Coinbase"
              />
              <ClassificationSelect
                label="Outbound Classification"
                value={() => draft().services.trading!.default_output_class}
                onChange={(v) => updateTradingConfig('default_output_class', v)}
                hint="Classification for data sent to Coinbase"
              />
            </div>
            <div class="channel-form-group">
              <label style={{ display: 'flex', 'align-items': 'center', gap: '0.5rem', cursor: 'pointer' }}>
                <input
                  type="checkbox"
                  checked={draft().services.trading!.sandbox}
                  onChange={(e) => updateTradingConfig('sandbox', e.currentTarget.checked)}
                />
                <span style={{ 'font-size': '0.88rem', color: 'hsl(var(--foreground))' }}>Sandbox mode</span>
              </label>
              <span class="channel-form-hint">Use the Coinbase sandbox API for testing (no real funds)</span>
            </div>
          </ServiceAccordion>
        </Show>
      </div>
    );
  }

  function renderReviewStep() {
    return (
      <div>
        <div class="channel-summary">
          <div class="channel-summary-row">
            <span class="channel-summary-label">Provider</span>
            <span class="channel-summary-value">{providerCard(draft().provider)?.icon} {providerCard(draft().provider)?.title}</span>
          </div>
          <div class="channel-summary-row">
            <span class="channel-summary-label">Name</span>
            <span class="channel-summary-value">{draft().name || '(unnamed)'}</span>
          </div>
          <div class="channel-summary-row">
            <span class="channel-summary-label">Services</span>
            <span class="channel-summary-value">
              {enabledServiceList(draft()).map((s) => `${SERVICE_INFO[s].icon} ${SERVICE_INFO[s].label}`).join(', ')}
            </span>
          </div>
          <div class="channel-summary-row">
            <span class="channel-summary-label">Auth Type</span>
            <span class="channel-summary-value">{draft().auth.type}</span>
          </div>
          <Show when={draft().services.communication}>
            <div class="channel-summary-row">
              <span class="channel-summary-label">Communication</span>
              <span class="channel-summary-value">
                {draft().services.communication!.from_address || draft().services.communication!.listen_channel_ids.join(', ') || 'Configured'}
              </span>
            </div>
          </Show>
        </div>
        {/* Allowed Personas */}
        <div style={{ 'margin-top': '1rem' }}>
          <div style={{ 'font-weight': '600', 'margin-bottom': '0.5rem' }}>Allowed Personas</div>
          <p style={{ 'font-size': '0.85rem', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '0.5rem' }}>
            Select which personas can access this connector.
          </p>
          <PersonaSelector
            multiple
            values={selectedPersonas()}
            onChange={setSelectedPersonas}
            personas={availablePersonas()}
          />
        </div>
        <div style={{ display: 'flex', 'align-items': 'center', gap: '0.75rem', 'margin-top': '0.75rem' }}>
            <button onClick={() => props.onTest(draft().id, draft())} disabled={props.testing()}>
              {props.testing() ? 'Testing…' : 'Test Connection'}
            </button>
            <Show when={!props.testing() && props.testResult()}>
              <span style={{
                'font-size': '0.85rem',
                color: props.testResult()!.startsWith('Error') ? 'hsl(var(--destructive))' : 'hsl(160 60% 76%)',
              }}>
                {props.testResult()!.startsWith('Error') ? '✗' : '✓'} {props.testResult()}
              </span>
            </Show>
        </div>
      </div>
    );
  }

  // ── Main render ──────────────────────────────────────────────

  return (
    <Dialog open={true} onOpenChange={(open) => { if (!open) props.onClose(); }}>
      <DialogContent class="max-w-[700px] w-[90vw] max-h-[85vh] flex flex-col overflow-hidden p-0" onInteractOutside={(e) => e.preventDefault()}>
        {/* Header with step indicators */}
        <div class="channel-wizard-header">
          <h2>Add Connector</h2>
          <div class="wizard-steps">
            <For each={wizardSteps()}>
              {(step, i) => (
                <>
                  <Show when={i() > 0}>
                    <div class="wizard-step-line" classList={{ completed: i() <= currentStepIdx() }} />
                  </Show>
                  <div
                    class="wizard-step"
                    classList={{
                      active: i() === currentStepIdx(),
                      completed: i() < currentStepIdx(),
                    }}
                  >
                    <div class="wizard-step-num">
                      {i() < currentStepIdx() ? '✓' : i() + 1}
                    </div>
                    <span class="wizard-step-label">{STEP_LABELS[step]}</span>
                  </div>
                </>
              )}
            </For>
          </div>
        </div>

        {/* Body */}
        <div class="channel-wizard-body">
          <Show when={wizardStep() === 'provider'}>
            {(_: any) => renderProviderStep()}
          </Show>
          <Show when={wizardStep() === 'services'}>
            {(_: any) => renderServicesStep()}
          </Show>
          <Show when={wizardStep() === 'connect'}>
            {(_: any) => renderConnectStep()}
          </Show>
          <Show when={wizardStep() === 'configure'}>
            {(_: any) => renderConfigureStep()}
          </Show>
          <Show when={wizardStep() === 'review'}>
            {(_: any) => renderReviewStep()}
          </Show>
        </div>

        {/* Footer */}
        <div class="channel-wizard-footer">
          <div style={{ display: 'flex', gap: '0.5rem' }}>
            <Button variant="outline" onClick={props.onClose}>Cancel</Button>
            <Show when={wizardStep() !== 'provider'}>
              <Button variant="outline" onClick={prevWizardStep}>← Back</Button>
            </Show>
          </div>
          <Show when={wizardStep() !== 'review'} fallback={
            <Button onClick={() => props.onFinish({ ...draft(), allowed_personas: selectedPersonas().length > 0 ? selectedPersonas() : undefined })} disabled={props.saving()}>
              {props.saving() ? 'Creating…' : 'Create Connector'}
            </Button>
          }>
            <Show when={wizardStep() !== 'provider'}>
              <Button onClick={nextWizardStep} disabled={!canAdvance()}>
                Next →
              </Button>
            </Show>
          </Show>
        </div>
      </DialogContent>
    </Dialog>
  );
}
