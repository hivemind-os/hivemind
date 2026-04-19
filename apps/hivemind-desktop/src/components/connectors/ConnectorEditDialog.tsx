import { ErrorBoundary, For, Show, createSignal, onMount } from 'solid-js';
import { PersonaSelector, type PersonaInfo } from '../shared';
import {
  type AuthConfig,
  type CalendarConfig,
  type CommunicationConfig,
  type ConnectorConfig,
  type ContactsConfig,
  type DriveConfig,
  type OAuthState,
  type ResourceRule,
  type ServiceType,
  type TradingConfig,
  SERVICE_INFO,
  defaultCalendarConfig,
  defaultCommConfig,
  defaultContactsConfig,
  defaultDriveConfig,
  defaultTradingConfig,
  enabledServiceList,
  providerCard,
  providerServices,
} from './types';
import { invoke } from '@tauri-apps/api/core';
import { ClassificationSelect, CommConfigForm, OAuthFlow, ServiceAccordion } from './shared';
import { Dialog, DialogContent, DialogTitle } from '~/ui/dialog';
// Tabs handled via plain buttons + signal — Kobalte tabs had click issues
import { Button } from '~/ui/button';

// ── Props ────────────────────────────────────────────────────────

interface ConnectorEditDialogProps {
  connector: ConnectorConfig;
  oauth: OAuthState;
  saving: () => boolean;
  testing: () => boolean;
  testResult: () => string | null;
  onTest: (id: string) => Promise<void>;
  onSave: (config: ConnectorConfig) => Promise<void>;
  onClose: () => void;
}

// ── Component ────────────────────────────────────────────────────

export function ConnectorEditDialog(props: ConnectorEditDialogProps) {
  // Hydrate missing array/object fields that the backend omits via skip_serializing_if
  function hydrateConnector(c: ConnectorConfig): ConnectorConfig {
    const raw = JSON.parse(JSON.stringify(c)) as ConnectorConfig;
    const comm = raw.services?.communication;
    if (comm) {
      comm.listen_channel_ids ??= [];
      comm.allowed_guild_ids ??= [];
      comm.destination_rules ??= [];
    }
    return raw;
  }

  const [editDraft, setEditDraft] = createSignal<ConnectorConfig>(hydrateConnector(props.connector));
  const [editTab, setEditTab] = createSignal<'services' | 'connection' | 'classification' | 'personas'>('services');

  // Persona access control
  const [availablePersonas, setAvailablePersonas] = createSignal<{ id: string; name: string }[]>([]);
  const [selectedPersonas, setSelectedPersonas] = createSignal<string[]>(
    props.connector.allowed_personas ? [...props.connector.allowed_personas] : [],
  );

  onMount(() => {
    invoke<{ id: string; name: string }[]>('list_personas', { include_archived: false })
      .then((ps) => setAvailablePersonas(ps.map((p) => ({ id: p.id, name: p.name }))))
      .catch(() => {});
  });

  // ── Updater helpers ──────────────────────────────────────────

  function updateDraft(fn: (d: ConnectorConfig) => ConnectorConfig) {
    setEditDraft(fn(editDraft()));
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

  // ── Tab renderers ────────────────────────────────────────────

  function renderServicesTab() {
    return (
      <div class="flex flex-col gap-2">
        <For each={providerServices(editDraft().provider)}>
          {(svc) => {
            const info = SERVICE_INFO[svc];
            const enabled = () => {
              const s = editDraft().services[svc];
              return s ? s.enabled : false;
            };
            return (
              <label class="flex items-center gap-3 p-2.5 bg-background/50 border rounded-md cursor-pointer" classList={{ 'border-primary/25': enabled(), 'border-muted/10': !enabled() }}>
                <input
                  type="checkbox"
                  checked={enabled()}
                  onChange={() => {
                    updateDraft((d) => {
                      const services = { ...d.services };
                      if (svc === 'communication') {
                        services.communication = services.communication
                          ? { ...services.communication, enabled: !services.communication.enabled }
                          : defaultCommConfig();
                      } else if (svc === 'calendar') {
                        services.calendar = services.calendar
                          ? { ...services.calendar, enabled: !services.calendar.enabled }
                          : defaultCalendarConfig();
                      } else if (svc === 'drive') {
                        services.drive = services.drive
                          ? { ...services.drive, enabled: !services.drive.enabled }
                          : defaultDriveConfig();
                      } else if (svc === 'contacts') {
                        services.contacts = services.contacts
                          ? { ...services.contacts, enabled: !services.contacts.enabled }
                          : defaultContactsConfig();
                      } else if (svc === 'trading') {
                        services.trading = services.trading
                          ? { ...services.trading, enabled: !services.trading.enabled }
                          : defaultTradingConfig();
                      }
                      return { ...d, services };
                    });
                  }}
                />
                <div>
                  <span class="text-sm font-semibold text-foreground">
                    {info.icon} {info.label}
                  </span>
                  <span class="text-xs text-muted-foreground ml-2">
                    {info.desc}
                  </span>
                </div>
              </label>
            );
          }}
        </For>
      </div>
    );
  }

  function renderConnectionTab() {
    return (
      <div>
        <div class="space-y-1 mb-3">
          <label class="text-sm font-medium text-foreground">Connector Name</label>
          <input
            type="text"
            value={editDraft().name}
            onInput={(e) => updateDraft((d) => ({ ...d, name: e.currentTarget.value }))}
          />
        </div>
        <div class="space-y-1 mb-3">
          <label class="text-sm font-medium text-foreground">Enabled</label>
          <label class="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={editDraft().enabled}
              onChange={() => updateDraft((d) => ({ ...d, enabled: !d.enabled }))}
            />
            <span class="text-sm text-muted-foreground">
              {editDraft().enabled ? 'Active' : 'Disabled'}
            </span>
          </label>
        </div>

        <Show when={editDraft().auth.type === 'password'}>
          <div class="grid grid-cols-2 gap-3">
            <div class="space-y-1 mb-3">
              <label class="text-sm font-medium text-foreground">Username</label>
              <input type="text" value={editDraft().auth.username || ''} onInput={(e) => updateAuth('username', e.currentTarget.value)} />
            </div>
            <div class="space-y-1 mb-3">
              <label class="text-sm font-medium text-foreground">Password</label>
              <input type="password" value={editDraft().auth.password || ''} onInput={(e) => updateAuth('password', e.currentTarget.value)} placeholder="(stored securely — leave blank to keep current)" />
            </div>
          </div>
          <div class="grid grid-cols-2 gap-3">
            <div class="space-y-1 mb-3">
              <label class="text-sm font-medium text-foreground">IMAP Host</label>
              <input type="text" value={editDraft().auth.imap_host || ''} onInput={(e) => updateAuth('imap_host', e.currentTarget.value)} />
            </div>
            <div class="space-y-1 mb-3">
              <label class="text-sm font-medium text-foreground">IMAP Port</label>
              <input type="number" value={editDraft().auth.imap_port ?? 993} onInput={(e) => updateAuth('imap_port', parseInt(e.currentTarget.value) || 993)} />
            </div>
          </div>
          <div class="grid grid-cols-2 gap-3">
            <div class="space-y-1 mb-3">
              <label class="text-sm font-medium text-foreground">SMTP Host</label>
              <input type="text" value={editDraft().auth.smtp_host || ''} onInput={(e) => updateAuth('smtp_host', e.currentTarget.value)} />
            </div>
            <div class="space-y-1 mb-3">
              <label class="text-sm font-medium text-foreground">SMTP Port</label>
              <input type="number" value={editDraft().auth.smtp_port ?? 587} onInput={(e) => updateAuth('smtp_port', parseInt(e.currentTarget.value) || 587)} />
            </div>
          </div>
          <div class="space-y-1 mb-3">
            <label class="text-sm font-medium text-foreground">SMTP Encryption</label>
            <select
              value={editDraft().auth.smtp_encryption || 'starttls'}
              onChange={(e) => {
                const mode = e.currentTarget.value as 'starttls' | 'implicit-tls';
                updateAuth('smtp_encryption', mode);
                if (mode === 'implicit-tls' && (editDraft().auth.smtp_port === 587 || !editDraft().auth.smtp_port)) {
                  updateAuth('smtp_port', 465);
                } else if (mode === 'starttls' && editDraft().auth.smtp_port === 465) {
                  updateAuth('smtp_port', 587);
                }
              }}
            >
              <option value="starttls">STARTTLS (port 587)</option>
              <option value="implicit-tls">Implicit TLS / SMTPS (port 465)</option>
            </select>
          </div>
        </Show>

        <Show when={editDraft().auth.type === 'bot-token'}>
          <div class="space-y-1 mb-3">
            <label class="text-sm font-medium text-foreground">Bot Token</label>
            <input type="password" value={editDraft().auth.bot_token || ''} onInput={(e) => updateAuth('bot_token', e.currentTarget.value)} placeholder="(stored securely — leave blank to keep current)" />
          </div>
          <Show when={editDraft().provider === 'slack'}>
            <div class="space-y-1 mb-3">
              <label class="text-sm font-medium text-foreground">App-Level Token</label>
              <input type="password" value={editDraft().auth.app_token || ''} onInput={(e) => updateAuth('app_token', e.currentTarget.value)} placeholder="(stored securely — leave blank to keep current)" />
            </div>
          </Show>
        </Show>

        <Show when={editDraft().auth.type === 'oauth2'}>
          <div class="space-y-1 mb-3">
            <label class="text-sm font-medium text-foreground">Authentication</label>
            <OAuthFlow connectorId={editDraft().id} provider={editDraft().provider} oauth={props.oauth} />
          </div>
        </Show>

        <Show when={editDraft().auth.type === 'cdp-api-key'}>
          <div class="space-y-1 mb-3">
            <label class="text-sm font-medium text-foreground">API Key Name</label>
            <input type="text" value={editDraft().auth.key_name || ''} onInput={(e) => updateAuth('key_name', e.currentTarget.value)} placeholder="organizations/{org_id}/apiKeys/{key_id}" />
          </div>
          <div class="space-y-1 mb-3">
            <label class="text-sm font-medium text-foreground">Private Key (PEM)</label>
            <textarea
              value={editDraft().auth.private_key || ''}
              onInput={(e) => updateAuth('private_key', e.currentTarget.value)}
              placeholder={props.connector.auth.type === 'cdp-api-key' && props.connector.auth.key_name ? '(stored securely — leave blank to keep current key)' : '-----BEGIN EC PRIVATE KEY-----'}
              rows={4}
              style={{ 'font-family': 'monospace', 'font-size': '0.78rem', 'white-space': 'pre', resize: 'vertical' }}
            />
          </div>
        </Show>
      </div>
    );
  }

  function renderClassificationTab() {
    const services = () => enabledServiceList(editDraft());

    return (
      <div class="flex flex-col gap-2">
        <For each={services()}>
          {(svc) => (
            <ServiceAccordion service={svc}>
              <Show when={svc === 'communication' && editDraft().services.communication}>
                {(_: any) => (
                  <CommConfigForm
                    config={() => editDraft().services.communication!}
                    provider={() => editDraft().provider}
                    onUpdate={(k, v) => updateCommConfig(k as any, v)}
                    onAddRule={addDestinationRule}
                    onRemoveRule={removeDestinationRule}
                    onUpdateRule={updateDestinationRule}
                  />
                )}
              </Show>
              <Show when={svc === 'calendar' && editDraft().services.calendar}>
                {(_: any) => (
                  <ClassificationSelect
                    label="Default Classification"
                    value={() => editDraft().services.calendar!.default_class}
                    onChange={(v) => updateCalendarConfig('default_class', v)}
                    hint="Classification level for calendar data"
                  />
                )}
              </Show>
              <Show when={svc === 'drive' && editDraft().services.drive}>
                {(_: any) => (
                  <ClassificationSelect
                    label="Default Classification"
                    value={() => editDraft().services.drive!.default_class}
                    onChange={(v) => updateDriveConfig('default_class', v)}
                    hint="Classification level for file data"
                  />
                )}
              </Show>
              <Show when={svc === 'contacts' && editDraft().services.contacts}>
                {(_: any) => (
                  <ClassificationSelect
                    label="Default Classification"
                    value={() => editDraft().services.contacts!.default_class}
                    onChange={(v) => updateContactsConfig('default_class', v)}
                    hint="Classification level for contact data"
                  />
                )}
              </Show>
              <Show when={svc === 'trading' && editDraft().services.trading}>
                {(_: any) => (
                  <>
                    <div class="channel-form-row">
                      <ClassificationSelect
                        label="Inbound Classification"
                        value={() => editDraft().services.trading!.default_input_class}
                        onChange={(v) => updateTradingConfig('default_input_class', v)}
                        hint="Classification for data read from Coinbase"
                      />
                      <ClassificationSelect
                        label="Outbound Classification"
                        value={() => editDraft().services.trading!.default_output_class}
                        onChange={(v) => updateTradingConfig('default_output_class', v)}
                        hint="Classification for data sent to Coinbase"
                      />
                    </div>
                    <div class="channel-form-group" style={{ 'margin-top': '0.5rem' }}>
                      <label style={{ display: 'flex', 'align-items': 'center', gap: '0.5rem', cursor: 'pointer' }}>
                        <input
                          type="checkbox"
                          checked={editDraft().services.trading!.sandbox}
                          onChange={(e) => updateTradingConfig('sandbox', e.currentTarget.checked)}
                        />
                        <span style={{ 'font-size': '0.88rem', color: 'hsl(var(--foreground))' }}>Sandbox mode</span>
                      </label>
                      <span class="channel-form-hint">Use the Coinbase sandbox API for testing (no real funds)</span>
                    </div>
                  </>
                )}
              </Show>
            </ServiceAccordion>
          )}
        </For>
        <Show when={services().length === 0}>
          <p class="text-muted-foreground text-sm text-center p-8">
            No services enabled. Enable services in the Services tab.
          </p>
        </Show>
      </div>
    );
  }

  // ── Main render ──────────────────────────────────────────────

  return (
    <Dialog open={true} onOpenChange={(open) => { if (!open) props.onClose(); }}>
      <DialogContent class="max-w-[650px] w-[90vw] max-h-[80vh] flex flex-col p-0" onInteractOutside={(e) => e.preventDefault()}>
        <div class="flex items-center justify-between px-6 pt-6 pb-2">
          <DialogTitle class="flex items-center gap-2">{providerCard(editDraft().provider)?.icon} {editDraft().name || 'Edit Connector'}</DialogTitle>
          <button class="text-muted-foreground hover:text-foreground" onClick={props.onClose}>✕</button>
        </div>

        <div class="flex gap-1 bg-muted rounded-md p-1 mx-6 flex-none">
          {(['services', 'connection', 'classification', 'personas'] as const).map((tab) => (
            <button
              type="button"
              class={`flex-1 py-1.5 px-3 text-sm font-medium rounded-sm transition-all ${editTab() === tab ? 'bg-background text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground'}`}
              onClick={() => setEditTab(tab)}
            >
              {tab.charAt(0).toUpperCase() + tab.slice(1)}
            </button>
          ))}
        </div>

        <div class="flex-1 overflow-y-auto px-6 py-4">
          <ErrorBoundary fallback={(err) => <p class="text-destructive text-sm p-4">Render error: {String(err)}</p>}>
            <Show when={editTab() === 'services'}>
              {renderServicesTab()}
            </Show>
            <Show when={editTab() === 'connection'}>
              {renderConnectionTab()}
            </Show>
            <Show when={editTab() === 'classification'}>
              {renderClassificationTab()}
            </Show>
            <Show when={editTab() === 'personas'}>
              <div>
                <p style={{ 'font-size': '0.85rem', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '0.75rem' }}>
                  Select which personas can access this connector.
                </p>
                <PersonaSelector
                  multiple
                  values={selectedPersonas()}
                  onChange={setSelectedPersonas}
                  personas={availablePersonas()}
                />
                <Show when={selectedPersonas().length === 0}>
                  <p style={{ 'font-size': '0.82rem', color: 'hsl(var(--muted-foreground))', 'margin-top': '0.5rem', 'font-style': 'italic' }}>
                    No restrictions — all personas can use this connector.
                  </p>
                </Show>
              </div>
            </Show>
          </ErrorBoundary>
        </div>

        <div class="flex items-center justify-between px-6 pb-6 pt-2 border-t border-border">
          <div class="flex gap-2 items-center">
            <Show when={editDraft().provider !== 'apple'}>
              <Button variant="outline" size="sm" onClick={() => props.onTest(editDraft().id)} disabled={props.testing()}>
                {props.testing() ? 'Testing…' : 'Test'}
              </Button>
              <Show when={!props.testing() && props.testResult()}>
                <span style={{
                  'font-size': '0.82rem',
                  color: props.testResult()!.startsWith('Error') ? 'hsl(var(--destructive))' : 'hsl(160 60% 76%)',
                }}>
                  {props.testResult()!.startsWith('Error') ? '✗' : '✓'} {props.testResult()}
                </span>
              </Show>
            </Show>
          </div>
          <div class="flex gap-2">
            <Button variant="outline" onClick={props.onClose}>Cancel</Button>
            <Button onClick={() => props.onSave({ ...editDraft(), allowed_personas: selectedPersonas().length > 0 ? selectedPersonas() : undefined })} disabled={props.saving()}>
              {props.saving() ? 'Saving…' : 'Save Changes'}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
