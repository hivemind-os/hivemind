import { createSignal, onMount, For, Show } from 'solid-js';
import { Mail, Calendar, FolderClosed, User, RefreshCw, LoaderCircle } from 'lucide-solid';
import { Button, Badge } from '~/ui';
import { authFetch } from '~/lib/authFetch';

interface ServiceAuditEntry {
  id: string;
  connector_id: string;
  provider: string;
  service_type: string;
  operation: string;
  direction?: string | null;
  from_address?: string | null;
  to_address?: string | null;
  subject?: string | null;
  resource_id?: string | null;
  resource_name?: string | null;
  body_hash: string;
  body_preview?: string | null;
  data_class: string;
  approval_decision?: string | null;
  agent_id?: string | null;
  session_id?: string | null;
  timestamp_ms: number;
}

interface AuditViewerProps {
  daemon_url: string;
}

export default function AuditViewer(props: AuditViewerProps) {
  const [entries, setEntries] = createSignal<ServiceAuditEntry[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [filterConnector, setFilterConnector] = createSignal('');
  const [filterServiceType, setFilterServiceType] = createSignal('');
  const [filterDirection, setFilterDirection] = createSignal('');
  const [filterAgent, setFilterAgent] = createSignal('');

  const loadAudit = async () => {
    setLoading(true);
    try {
      const params = new URLSearchParams();
      if (filterConnector()) params.set('connector_id', filterConnector());
      if (filterServiceType()) params.set('service_type', filterServiceType());
      if (filterDirection()) params.set('direction', filterDirection());
      if (filterAgent()) params.set('agent_id', filterAgent());
      params.set('limit', '100');

      const resp = await authFetch(`${props.daemon_url}/api/v1/comms/audit?${params}`);
      if (resp.ok) {
        const data = await resp.json();
        const list = Array.isArray(data) ? data : data.results || data.entries || [];
        setEntries(list.map((e: any) => ({
          id: e.id,
          connector_id: e.connector_id || e.channel_id || '',
          provider: e.provider || e.channel_type || '',
          service_type: e.service_type || 'communication',
          operation: e.operation || (e.direction === 'outbound' ? 'send' : 'read'),
          direction: e.direction,
          from_address: e.from_address,
          to_address: e.to_address,
          subject: e.subject,
          resource_id: e.resource_id,
          resource_name: e.resource_name,
          body_hash: e.body_hash || '',
          body_preview: e.body_preview,
          data_class: e.data_class || 'internal',
          approval_decision: e.approval_decision,
          agent_id: e.agent_id,
          session_id: e.session_id,
          timestamp_ms: e.timestamp_ms || 0,
        })));
      }
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  };

  onMount(loadAudit);

  const formatTime = (ms: number) => {
    if (!ms) return '—';
    return new Date(ms).toLocaleString();
  };

  const serviceIcon = (svc: string) => {
    switch (svc) {
      case 'communication': return <Mail size={14} />;
      case 'calendar': return <Calendar size={14} />;
      case 'drive': return <FolderClosed size={14} />;
      case 'contacts': return <User size={14} />;
      default: return '•';
    }
  };

  return (
    <div>
      <div class="mb-3 flex items-center justify-between">
        <h3 class="text-sm font-semibold">Service Audit Log</h3>
        <Button variant="secondary" size="sm" onClick={loadAudit} disabled={loading()}>
          {loading() ? <LoaderCircle size={14} class="animate-spin" /> : <RefreshCw size={14} />} Refresh
        </Button>
      </div>

      {/* Filters */}
      <div class="mb-3 flex flex-wrap gap-2">
        <input
          placeholder="Connector ID"
          value={filterConnector()}
          onInput={e => setFilterConnector(e.currentTarget.value)}
          class="min-w-[120px] flex-1 rounded border border-input bg-transparent px-2 py-1 text-sm"
        />
        <select
          value={filterServiceType()}
          onChange={e => setFilterServiceType(e.currentTarget.value)}
          class="rounded border border-input bg-transparent px-2 py-1 text-sm"
        >
          <option value="">All services</option>
          <option value="communication">Communication</option>
          <option value="calendar">Calendar</option>
          <option value="drive">Drive</option>
          <option value="contacts">Contacts</option>
        </select>
        <select
          value={filterDirection()}
          onChange={e => setFilterDirection(e.currentTarget.value)}
          class="rounded border border-input bg-transparent px-2 py-1 text-sm"
        >
          <option value="">All directions</option>
          <option value="inbound">↓ Inbound</option>
          <option value="outbound">↑ Outbound</option>
        </select>
        <input
          placeholder="Agent ID"
          value={filterAgent()}
          onInput={e => setFilterAgent(e.currentTarget.value)}
          class="min-w-[100px] flex-1 rounded border border-input bg-transparent px-2 py-1 text-sm"
        />
        <Button size="sm" variant="outline" onClick={loadAudit}>Apply</Button>
      </div>

      {/* Table */}
      <div class="overflow-x-auto">
        <table class="w-full border-collapse text-xs">
          <thead>
            <tr class="border-b-2 border-input text-left">
              <th class="px-2 py-1.5">Time</th>
              <th class="px-2 py-1.5">Service</th>
              <th class="px-2 py-1.5">Connector</th>
              <th class="px-2 py-1.5">Operation</th>
              <th class="px-2 py-1.5">Dir</th>
              <th class="px-2 py-1.5">From / To</th>
              <th class="px-2 py-1.5">Subject / Resource</th>
              <th class="px-2 py-1.5">Class</th>
              <th class="px-2 py-1.5">Approval</th>
            </tr>
          </thead>
          <tbody>
            <For each={entries()} fallback={
              <tr><td colspan="9" class="py-5 text-center text-muted-foreground">
                {loading() ? 'Loading…' : 'No audit entries found.'}
              </td></tr>
            }>
              {(entry) => (
                <tr class="border-b border-input">
                  <td class="whitespace-nowrap px-2 py-1">{formatTime(entry.timestamp_ms)}</td>
                  <td class="px-2 py-1" title={entry.service_type}>
                    {serviceIcon(entry.service_type)}
                  </td>
                  <td class="max-w-[120px] truncate px-2 py-1" title={entry.connector_id}>
                    {entry.connector_id}
                  </td>
                  <td class="px-2 py-1">{entry.operation}</td>
                  <td class="px-2 py-1">
                    <Show when={entry.direction}>
                      <Badge variant={entry.direction === 'outbound' ? 'outline' : 'secondary'}>
                        {entry.direction === 'outbound' ? '↑' : '↓'}
                      </Badge>
                    </Show>
                  </td>
                  <td class="max-w-[180px] truncate px-2 py-1" title={`${entry.from_address || ''} → ${entry.to_address || ''}`}>
                    {entry.from_address || entry.resource_name || '—'}
                  </td>
                  <td class="max-w-[200px] truncate px-2 py-1" title={entry.subject || entry.resource_name || ''}>
                    {entry.subject || entry.resource_name || '—'}
                  </td>
                  <td class="px-2 py-1">
                    <Badge variant="secondary">{entry.data_class}</Badge>
                  </td>
                  <td class="px-2 py-1">
                    {entry.approval_decision || '—'}
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </div>
  );
}
