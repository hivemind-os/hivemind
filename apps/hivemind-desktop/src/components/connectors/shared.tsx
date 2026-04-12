import { For, Index, Show, createSignal, type JSX } from 'solid-js';
import { Collapsible, CollapsibleTrigger, CollapsibleContent } from '~/ui/collapsible';
import { openExternal } from '../../utils';
import {
  type ApprovalKind,
  type CommunicationConfig,
  type ConnectorConfig,
  type ConnectorProvider,
  type DataClassification,
  type OAuthState,
  type ResourceRule,
  type ServiceType,
  APPROVAL_OPTIONS,
  DATA_CLASS_OPTIONS,
  SERVICE_INFO,
  enabledServiceList,
  providerCard,
} from './types';

// ── OAuth Flow ───────────────────────────────────────────────────

export function OAuthFlow(props: {
  connectorId: string;
  provider: ConnectorProvider;
  oauth: OAuthState;
}) {
  function handleStart() {
    props.oauth.start(props.connectorId, props.provider);
  }

  return (
    <div class="channel-form-group">
      <Show when={props.oauth.status() === 'idle' || props.oauth.status() === 'error'}>
        <button
          class="primary"
          onClick={handleStart}
        >
          Authenticate with {providerCard(props.provider)?.title ?? props.provider}
        </button>
        <Show when={props.oauth.error()}>
          <p style={{ color: 'hsl(var(--destructive))', 'font-size': '0.85rem', 'white-space': 'pre-wrap' }}>{props.oauth.error()}</p>
        </Show>
      </Show>
      <Show when={props.oauth.status() === 'waiting'}>
        <p style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85rem' }}>Starting OAuth flow…</p>
      </Show>
      <Show when={props.oauth.status() === 'polling'}>
        <Show when={props.oauth.userCode()} fallback={
          <div style={{ 'text-align': 'center' }}>
            <p style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85rem' }}>A browser window has opened for authorization.</p>
            <p style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.78rem' }}>Waiting for you to complete sign-in…</p>
          </div>
        }>
          <div style={{ 'text-align': 'center', padding: '1rem 0' }}>
            <p style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85rem', 'margin-bottom': '0.75rem' }}>
              Open the verification URL and enter this code:
            </p>
            <div style={{
              'font-size': '1.8rem', 'font-weight': '700', 'letter-spacing': '0.15em',
              color: 'hsl(var(--primary))', padding: '0.75rem 1.5rem',
              background: 'hsl(var(--primary) / 0.08)',
              'border-radius': '0.75rem', display: 'inline-block', 'margin-bottom': '0.75rem',
            }}>
              {props.oauth.userCode()}
            </div>
            <Show when={props.oauth.verifyUrl()}>
              <p style={{ 'font-size': '0.82rem' }}>
                <a
                  href={props.oauth.verifyUrl()!}
                  onClick={(e) => { e.preventDefault(); openExternal(props.oauth.verifyUrl()!); }}
                  style={{ color: 'hsl(var(--primary))', cursor: 'pointer' }}
                >
                  {props.oauth.verifyUrl()}
                </a>
              </p>
            </Show>
            <p style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.78rem', 'margin-top': '0.5rem' }}>
              Waiting for authorization…
            </p>
          </div>
        </Show>
        <button onClick={props.oauth.reset} style={{ 'margin-top': '0.5rem' }}>Cancel</button>
      </Show>
      <Show when={props.oauth.status() === 'complete'}>
        <div style={{ 'text-align': 'center', padding: '0.75rem 0' }}>
          <span style={{ 'font-size': '1.5rem' }}>✅</span>
          <p style={{ color: 'hsl(160 60% 76%)', 'font-size': '0.92rem', 'font-weight': '600', 'margin-top': '0.25rem' }}>
            Connected successfully!
          </p>
        </div>
      </Show>
    </div>
  );
}

// ── Classification Select ────────────────────────────────────────

export function ClassificationSelect(props: {
  label: string;
  value: () => DataClassification;
  onChange: (v: DataClassification) => void;
  hint?: string;
}) {
  return (
    <div class="channel-form-group">
      <label class="channel-form-label">{props.label}</label>
      <select value={props.value()} onChange={(e) => props.onChange(e.currentTarget.value as DataClassification)}>
        <For each={DATA_CLASS_OPTIONS}>
          {(opt) => <option value={opt}>{opt}</option>}
        </For>
      </select>
      <Show when={props.hint}>
        <span class="channel-form-hint">{props.hint}</span>
      </Show>
    </div>
  );
}

// ── Destination Rules ────────────────────────────────────────────

export function DestinationRules(props: {
  rules: () => ResourceRule[];
  onAdd: () => void;
  onRemove: (idx: number) => void;
  onUpdate: (idx: number, key: keyof ResourceRule, value: any) => void;
}) {
  return (
    <div class="channel-form-group">
      <label class="channel-form-label">Destination Rules</label>
      <span class="channel-form-hint">Pattern-based rules for message destinations</span>
      <Index each={props.rules()}>
        {(rule, idx) => (
          <div style={{ display: 'flex', gap: '0.5rem', 'align-items': 'center', 'margin-top': '0.35rem' }}>
            <input
              type="text"
              value={rule().pattern}
              onInput={(e) => props.onUpdate(idx, 'pattern', e.currentTarget.value)}
              placeholder="*@example.com"
              style={{ flex: '1' }}
            />
            <select
              value={rule().approval}
              onChange={(e) => props.onUpdate(idx, 'approval', e.currentTarget.value as ApprovalKind)}
              style={{ width: '6rem' }}
            >
              <For each={APPROVAL_OPTIONS}>
                {(opt) => <option value={opt}>{opt}</option>}
              </For>
            </select>
            <button
              class="btn-danger"
              onClick={() => props.onRemove(idx)}
              style={{ padding: '0.25rem 0.5rem', 'font-size': '0.78rem' }}
            >
              ✕
            </button>
          </div>
        )}
      </Index>
      <button
        onClick={props.onAdd}
        style={{ 'margin-top': '0.5rem', 'font-size': '0.82rem' }}
      >
        + Add Rule
      </button>
    </div>
  );
}

// ── Communication Config Form ────────────────────────────────────

export function CommConfigForm(props: {
  config: () => CommunicationConfig;
  provider: () => ConnectorProvider;
  onUpdate: (key: keyof CommunicationConfig, value: any) => void;
  onAddRule: () => void;
  onRemoveRule: (idx: number) => void;
  onUpdateRule: (idx: number, key: keyof ResourceRule, value: any) => void;
}) {
  const isEmailProvider = () => {
    const p = props.provider();
    return p === 'microsoft' || p === 'gmail' || p === 'imap';
  };
  const isDiscordOrSlack = () => {
    const p = props.provider();
    return p === 'discord' || p === 'slack';
  };

  return (
    <div>
      <Show when={isEmailProvider()}>
        <div class="channel-form-row">
          <div class="channel-form-group">
            <label class="channel-form-label">From Address</label>
            <input
              type="text"
              value={props.config().from_address || ''}
              onInput={(e) => props.onUpdate('from_address', e.currentTarget.value)}
              placeholder="user@example.com"
            />
          </div>
          <div class="channel-form-group">
            <label class="channel-form-label">Folder</label>
            <input
              type="text"
              value={props.config().folder}
              onInput={(e) => props.onUpdate('folder', e.currentTarget.value)}
              placeholder="INBOX"
            />
          </div>
        </div>
        <div class="channel-form-group">
          <label class="channel-form-label">Poll Interval (seconds)</label>
          <input
            type="number"
            value={props.config().poll_interval_secs ?? 60}
            onInput={(e) => props.onUpdate('poll_interval_secs', parseInt(e.currentTarget.value) || null)}
            min="10"
          />
          <span class="channel-form-hint">How often to check for new messages</span>
        </div>
      </Show>
      <Show when={isDiscordOrSlack()}>
        <div class="channel-form-group">
          <label class="channel-form-label">Listen Channel IDs</label>
          <input
            type="text"
            value={(props.config().listen_channel_ids || []).join(', ')}
            onInput={(e) => props.onUpdate('listen_channel_ids', e.currentTarget.value.split(',').map((s) => s.trim()).filter(Boolean))}
            placeholder="Comma-separated channel IDs"
          />
          <span class="channel-form-hint">Channels the bot will listen on</span>
        </div>
        <div class="channel-form-group">
          <label class="channel-form-label">Default Send Channel ID</label>
          <input
            type="text"
            value={props.config().default_send_channel_id || ''}
            onInput={(e) => props.onUpdate('default_send_channel_id', e.currentTarget.value || null)}
            placeholder="Channel ID for outgoing messages"
          />
        </div>
        <Show when={props.provider() === 'discord'}>
          <div class="channel-form-group">
            <label class="channel-form-label">Allowed Guild IDs</label>
            <input
              type="text"
              value={(props.config().allowed_guild_ids || []).join(', ')}
              onInput={(e) => props.onUpdate('allowed_guild_ids', e.currentTarget.value.split(',').map((s) => s.trim()).filter(Boolean))}
              placeholder="Comma-separated guild (server) IDs"
            />
          </div>
        </Show>
      </Show>
      <div class="channel-form-row">
        <ClassificationSelect
          label="Inbound Classification"
          value={() => props.config().default_input_class}
          onChange={(v) => props.onUpdate('default_input_class', v)}
        />
        <ClassificationSelect
          label="Outbound Classification"
          value={() => props.config().default_output_class}
          onChange={(v) => props.onUpdate('default_output_class', v)}
        />
      </div>
      <DestinationRules
        rules={() => props.config().destination_rules || []}
        onAdd={props.onAddRule}
        onRemove={props.onRemoveRule}
        onUpdate={props.onUpdateRule}
      />
    </div>
  );
}

// ── Service Accordion ────────────────────────────────────────────

export function ServiceAccordion(props: {
  service: ServiceType;
  children: JSX.Element;
}) {
  const [expanded, setExpanded] = createSignal(true);
  const info = SERVICE_INFO[props.service];

  return (
    <Collapsible open={expanded()} onOpenChange={setExpanded}>
      <div style={{
        border: '1px solid hsl(var(--border) / 0.1)',
        'border-radius': '0.75rem',
        overflow: 'hidden',
        'margin-bottom': '0.5rem',
      }}>
        <CollapsibleTrigger
          as="button"
          class="channel-advanced-toggle"
          style={{ width: '100%', padding: '0.75rem 1rem', 'justify-content': 'space-between' }}
        >
          <span style={{ 'font-weight': '600', color: 'hsl(var(--foreground))' }}>
            {info.icon} {info.label}
          </span>
          <span>{expanded() ? '▾' : '▸'}</span>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <div style={{ padding: '0.5rem 1rem 1rem' }}>
            {props.children}
          </div>
        </CollapsibleContent>
      </div>
    </Collapsible>
  );
}

// ── Service Badges ───────────────────────────────────────────────

export function ServiceBadges(props: { config: ConnectorConfig }) {
  const services = () => enabledServiceList(props.config);
  return (
    <div style={{ display: 'flex', gap: '0.35rem', 'flex-wrap': 'wrap' }}>
      <For each={services()}>
        {(svc) => (
          <span class="badge" title={SERVICE_INFO[svc].label} style={{ 'font-size': '0.9rem', padding: '0.15rem 0.35rem' }}>
            {SERVICE_INFO[svc].icon}
          </span>
        )}
      </For>
    </div>
  );
}
