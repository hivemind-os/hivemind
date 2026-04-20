import { slugifyId } from '../../utils';
import type { DataClassification, ApprovalKind } from '../../types';
import type { PluginConfigSchema } from '../plugins/PluginConfigForm';

// Re-export shared types for convenience
export type { DataClassification, ApprovalKind };

// ── Types ────────────────────────────────────────────────────────

export type ConnectorProvider = 'microsoft' | 'gmail' | 'imap' | 'discord' | 'slack' | 'apple' | 'coinbase';
export type ServiceType = 'communication' | 'calendar' | 'drive' | 'contacts' | 'trading';
export type WizardStep = 'provider' | 'services' | 'connect' | 'configure' | 'review';
export type OAuthStatus = 'idle' | 'waiting' | 'polling' | 'complete' | 'error';

// ── Plugin types (for unified display) ───────────────────────────

export interface InstalledPlugin {
  plugin_id: string;
  name: string;
  version: string;
  display_name: string;
  description: string;
  plugin_type: string;
  enabled: boolean;
  config: Record<string, any>;
  config_schema?: PluginConfigSchema | null;
  status?: { state: string; message?: string };
  permissions: string[];
  allowed_personas?: string[];
}

// ── Unified Integration type ─────────────────────────────────────

export type IntegrationKind = 'builtin' | 'plugin';

export interface Integration {
  id: string;
  kind: IntegrationKind;
  name: string;
  icon: string;
  description: string;
  enabled: boolean;
  status?: { state: string; message?: string };
  // Built-in connector data (present when kind === 'builtin')
  connector?: ConnectorConfig;
  // Plugin data (present when kind === 'plugin')
  plugin?: InstalledPlugin;
}

/** Convert a ConnectorConfig into the unified Integration type. */
export function connectorToIntegration(conn: ConnectorConfig, status?: ConnectorStatus): Integration {
  const card = providerCard(conn.provider);
  return {
    id: conn.id,
    kind: 'builtin',
    name: conn.name || card?.title || conn.provider,
    icon: card?.icon ?? '🔗',
    description: card?.desc ?? '',
    enabled: conn.enabled,
    status: status ? { state: status.state, message: status.message } : undefined,
    connector: conn,
  };
}

/** Convert an InstalledPlugin into the unified Integration type. */
export function pluginToIntegration(plugin: InstalledPlugin): Integration {
  return {
    id: `plugin:${plugin.plugin_id}`,
    kind: 'plugin',
    name: plugin.display_name || plugin.name,
    icon: '🧩',
    description: plugin.description || '',
    enabled: plugin.enabled,
    status: plugin.status ? { state: plugin.status.state, message: plugin.status.message } : undefined,
    plugin,
  };
}

export interface ResourceRule {
  pattern: string;
  approval: ApprovalKind;
  input_class_override?: DataClassification | null;
  output_class_override?: DataClassification | null;
}

export interface CommunicationConfig {
  enabled: boolean;
  from_address?: string;
  folder: string;
  poll_interval_secs?: number | null;
  default_input_class: DataClassification;
  default_output_class: DataClassification;
  destination_rules: ResourceRule[];
  allowed_guild_ids: string[];
  listen_channel_ids: string[];
  default_send_channel_id?: string | null;
}

export interface CalendarConfig {
  enabled: boolean;
  default_class: DataClassification;
  resource_rules: ResourceRule[];
}

export interface DriveConfig {
  enabled: boolean;
  default_class: DataClassification;
  resource_rules: ResourceRule[];
}

export interface ContactsConfig {
  enabled: boolean;
  default_class: DataClassification;
  resource_rules: ResourceRule[];
}

export interface TradingConfig {
  enabled: boolean;
  default_input_class: DataClassification;
  default_output_class: DataClassification;
  sandbox: boolean;
}

export interface ServicesConfig {
  communication?: CommunicationConfig | null;
  calendar?: CalendarConfig | null;
  drive?: DriveConfig | null;
  contacts?: ContactsConfig | null;
  trading?: TradingConfig | null;
}

export interface AuthConfig {
  type: 'oauth2' | 'password' | 'bot-token' | 'cdp-api-key' | 'local';
  client_id?: string;
  client_secret?: string;
  refresh_token?: string;
  access_token?: string;
  token_url?: string;
  username?: string;
  password?: string;
  imap_host?: string;
  imap_port?: number;
  smtp_host?: string;
  smtp_port?: number;
  smtp_encryption?: 'starttls' | 'implicit-tls';
  bot_token?: string;
  app_token?: string;
  key_name?: string;
  private_key?: string;
}

export interface ConnectorConfig {
  id: string;
  name: string;
  provider: ConnectorProvider;
  enabled: boolean;
  auth: AuthConfig;
  services: ServicesConfig;
  allowed_personas?: string[];
}

export interface ConnectorStatus {
  state: 'connected' | 'disconnected' | 'auth-expired' | 'error';
  message?: string;
}

export interface DiscoveredChannel {
  id: string;
  name: string;
  type?: string | number;
  guild_name?: string;
  is_member?: boolean;
  is_channel?: boolean;
  is_im?: boolean;
}

export interface DiscoveredGuild {
  id: string;
  name: string;
}

// ── State bundles passed to sub-components ───────────────────────

export interface OAuthState {
  status: () => OAuthStatus;
  userCode: () => string | null;
  verifyUrl: () => string | null;
  error: () => string | null;
  start: (connectorId: string, provider: ConnectorProvider, email?: string, services?: ServiceType[], clientId?: string, clientSecret?: string) => Promise<void>;
  reset: () => void;
}

export interface DiscoveryState {
  discovering: () => boolean;
  channels: () => DiscoveredChannel[];
  guilds: () => DiscoveredGuild[];
  workspace: () => string | null;
  error: () => string | null;
  discover: (connectorId: string, provider: ConnectorProvider, botToken: string) => Promise<void>;
}

// ── Constants ────────────────────────────────────────────────────

export const STEP_LABELS: Record<WizardStep, string> = {
  provider: 'Provider',
  services: 'Services',
  connect: 'Connect',
  configure: 'Configure',
  review: 'Review',
};

export const PROVIDER_CARDS: {
  provider: ConnectorProvider;
  icon: string;
  title: string;
  desc: string;
  enabled: boolean;
  services: ServiceType[];
  platform?: string;
}[] = [
  { provider: 'microsoft', icon: '🔷', title: 'Microsoft 365', desc: 'Outlook, Calendar, OneDrive, Contacts via Graph API', enabled: true, services: ['communication', 'calendar', 'drive', 'contacts'] },
  { provider: 'gmail', icon: '📧', title: 'Google Workspace', desc: 'Gmail, Calendar, Drive, Contacts via Google APIs', enabled: true, services: ['communication', 'calendar', 'drive', 'contacts'] },
  { provider: 'imap', icon: '📬', title: 'IMAP/SMTP', desc: 'Generic IMAP/SMTP email', enabled: true, services: ['communication'] },
  { provider: 'discord', icon: '🎮', title: 'Discord', desc: 'Bot integration with gateway', enabled: true, services: ['communication'] },
  { provider: 'slack', icon: '💬', title: 'Slack', desc: 'App with Socket Mode', enabled: true, services: ['communication'] },
  { provider: 'apple', icon: '🍎', title: 'Apple', desc: 'Local Calendar & Contacts via macOS frameworks', enabled: true, services: ['calendar', 'contacts'], platform: 'macos' },
  { provider: 'coinbase', icon: '🪙', title: 'Coinbase', desc: 'Crypto trading — balances, prices, buy/sell, send', enabled: true, services: ['trading'] },
];

export const SERVICE_INFO: Record<ServiceType, { icon: string; label: string; desc: string }> = {
  communication: { icon: '✉️', label: 'Communication', desc: 'Send and receive messages' },
  calendar: { icon: '📅', label: 'Calendar', desc: 'View and manage calendar events' },
  drive: { icon: '📁', label: 'Drive', desc: 'Access and manage files' },
  contacts: { icon: '👤', label: 'Contacts', desc: 'Search and view contacts' },
  trading: { icon: '📈', label: 'Trading', desc: 'Crypto balances, prices, orders & transfers' },
};

export const DATA_CLASS_OPTIONS: DataClassification[] = ['public', 'internal', 'confidential', 'restricted'];
export const APPROVAL_OPTIONS: ApprovalKind[] = ['auto', 'ask', 'deny'];

// ── Helpers ──────────────────────────────────────────────────────

export function providerCard(p: ConnectorProvider) {
  return PROVIDER_CARDS.find((c) => c.provider === p);
}

export function providerServices(p: ConnectorProvider): ServiceType[] {
  return providerCard(p)?.services ?? ['communication'];
}

export function isSingleServiceProvider(p: ConnectorProvider): boolean {
  return providerServices(p).length <= 1;
}

export function defaultAuth(p: ConnectorProvider): AuthConfig {
  switch (p) {
    case 'microsoft':
    case 'gmail':
      return { type: 'oauth2' };
    case 'imap':
      return { type: 'password', imap_host: '', imap_port: 993, smtp_host: '', smtp_port: 587, smtp_encryption: 'starttls' };
    case 'discord':
      return { type: 'bot-token', bot_token: '' };
    case 'slack':
      return { type: 'bot-token', bot_token: '', app_token: '' };
    case 'apple':
      return { type: 'local' };
    case 'coinbase':
      return { type: 'cdp-api-key', key_name: '', private_key: '' };
  }
}

export function defaultCommConfig(): CommunicationConfig {
  return {
    enabled: true,
    from_address: '',
    folder: 'INBOX',
    poll_interval_secs: 60,
    default_input_class: 'internal',
    default_output_class: 'internal',
    destination_rules: [],
    allowed_guild_ids: [],
    listen_channel_ids: [],
    default_send_channel_id: null,
  };
}

export function defaultCalendarConfig(): CalendarConfig {
  return { enabled: true, default_class: 'internal', resource_rules: [] };
}

export function defaultDriveConfig(): DriveConfig {
  return { enabled: true, default_class: 'internal', resource_rules: [] };
}

export function defaultContactsConfig(): ContactsConfig {
  return { enabled: true, default_class: 'internal', resource_rules: [] };
}

export function defaultTradingConfig(): TradingConfig {
  return { enabled: true, default_input_class: 'internal', default_output_class: 'internal', sandbox: false };
}

export function createEmptyConnector(provider: ConnectorProvider, existingIds: string[] = []): ConnectorConfig {
  const svcs = providerServices(provider);
  const services: ServicesConfig = {};
  if (svcs.includes('communication')) services.communication = defaultCommConfig();
  if (svcs.includes('calendar')) services.calendar = defaultCalendarConfig();
  if (svcs.includes('drive')) services.drive = defaultDriveConfig();
  if (svcs.includes('contacts')) services.contacts = defaultContactsConfig();
  if (svcs.includes('trading')) services.trading = defaultTradingConfig();
  return {
    id: slugifyId(provider, existingIds),
    name: '',
    provider,
    enabled: true,
    auth: defaultAuth(provider),
    services,
  };
}

export function enabledServiceList(cfg: ConnectorConfig): ServiceType[] {
  const result: ServiceType[] = [];
  if (cfg.services.communication?.enabled) result.push('communication');
  if (cfg.services.calendar?.enabled) result.push('calendar');
  if (cfg.services.drive?.enabled) result.push('drive');
  if (cfg.services.contacts?.enabled) result.push('contacts');
  if (cfg.services.trading?.enabled) result.push('trading');
  return result;
}

export function statusColor(s: ConnectorStatus['state']): string {
  switch (s) {
    case 'connected': return '#22c55e';
    case 'disconnected': return '#64748b';
    case 'auth-expired': return '#f59e0b';
    case 'error': return '#ef4444';
  }
}

export function getDiscordInviteUrl(botToken: string | undefined): string | null {
  if (!botToken) return null;
  try {
    const clientId = atob(botToken.split('.')[0]);
    return `https://discord.com/api/oauth2/authorize?client_id=${clientId}&permissions=274877975552&scope=bot`;
  } catch { return null; }
}
