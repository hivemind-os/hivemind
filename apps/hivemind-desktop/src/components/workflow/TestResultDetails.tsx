/**
 * TestResultDetailsDialog — Full-size dialog showing test run results.
 * Tabs: Steps | Output | Actions
 */
import { createSignal, For, Show } from 'solid-js';
import type { WorkflowTestResult, WorkflowTestFailure, StepStateSnapshot, InterceptedActionSnapshot } from '~/types';
import { Dialog, DialogContent, DialogHeader, DialogBody, DialogFooter, DialogTitle, Button } from '~/ui';

export interface TestResultDetailsDialogProps {
  result: WorkflowTestResult | null;
  open: boolean;
  onClose: () => void;
}

// ── Helpers ──────────────────────────────────────────────────────────────

function statusColor(status: string): string {
  if (status === 'completed') return '#34d399';
  if (status === 'failed') return '#f87171';
  if (status === 'running') return '#60a5fa';
  if (status === 'skipped') return '#94a3b8';
  if (status.startsWith('waiting')) return '#fbbf24';
  return 'hsl(var(--muted-foreground))';
}

function kindLabel(kind: string): string {
  if (kind === 'ask_user') return '💬 ask user';
  if (kind === 'tool_approval') return '🔐 tool approval';
  if (kind === 'tool_call') return '🔧 tool call';
  return kind.replace(/_/g, ' ');
}

function formatJson(val: unknown): string {
  if (val === undefined || val === null) return '—';
  if (typeof val === 'string') return val;
  try { return JSON.stringify(val, null, 2); } catch { return String(val); }
}

function summarizeDetails(details: Record<string, unknown>): string {
  const toolId = details.tool_id as string | undefined;
  const question = details.question as string | undefined;
  const autoResponse = details.auto_response as string | undefined;
  if (question) return question.length > 80 ? question.slice(0, 80) + '…' : question;
  if (toolId && autoResponse) return `${toolId} → auto-approved`;
  if (toolId) return toolId;
  const agentId = details.agent_id as string | undefined;
  if (agentId) return agentId;
  const keys = Object.keys(details);
  if (keys.length === 0) return '—';
  return keys.slice(0, 3).join(', ') + (keys.length > 3 ? '…' : '');
}

// ── Styles ───────────────────────────────────────────────────────────────

const tabBtn = (active: boolean): Record<string, string> => ({
  padding: '6px 14px',
  'font-size': '0.85em',
  'font-weight': active ? '600' : '400',
  'border-radius': '6px',
  border: 'none',
  cursor: 'pointer',
  background: active ? 'hsl(var(--primary))' : 'hsl(var(--muted))',
  color: active ? 'hsl(var(--primary-foreground))' : 'hsl(var(--foreground))',
  transition: 'background 0.15s',
});

const thStyle: Record<string, string> = {
  padding: '8px 12px',
  'font-weight': '600',
  'text-align': 'left',
  'font-size': '0.85em',
  'border-bottom': '2px solid hsl(var(--border))',
  color: 'hsl(var(--muted-foreground))',
  'white-space': 'nowrap',
};

const tdStyle: Record<string, string> = {
  padding: '6px 12px',
  'font-size': '0.875em',
  'border-bottom': '1px solid hsl(var(--border) / 0.5)',
  'vertical-align': 'top',
};

const monoBlock: Record<string, string> = {
  'font-family': 'monospace',
  'font-size': '0.875em',
  'white-space': 'pre-wrap',
  'word-break': 'break-all',
  background: 'hsl(var(--muted) / 0.4)',
  'border-radius': '6px',
  padding: '12px 16px',
  'max-height': '400px',
  'overflow-y': 'auto',
  'line-height': '1.5',
};

// ── Expandable row for step output ───────────────────────────────────────

function StepOutputCell(props: { ss: StepStateSnapshot }) {
  const [expanded, setExpanded] = createSignal(false);
  const text = () => formatJson(props.ss.outputs);
  const isLong = () => text().length > 100;

  return (
    <Show when={props.ss.error} fallback={
      <Show when={props.ss.outputs !== undefined && props.ss.outputs !== null} fallback={
        <span style={{ color: 'hsl(var(--muted-foreground))' }}>—</span>
      }>
        <div>
          <Show when={isLong() && !expanded()}>
            <span style={{ 'font-family': 'monospace', 'font-size': '0.92em' }}>{text().slice(0, 100)}…</span>
            <button onClick={() => setExpanded(true)} style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'hsl(var(--primary))', 'font-size': '0.85em', 'margin-left': '4px' }}>show</button>
          </Show>
          <Show when={!isLong() || expanded()}>
            <pre style={{ margin: '0', 'font-family': 'monospace', 'font-size': '0.92em', 'white-space': 'pre-wrap', 'word-break': 'break-all' }}>{text()}</pre>
            <Show when={isLong()}>
              <button onClick={() => setExpanded(false)} style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'hsl(var(--primary))', 'font-size': '0.85em', 'margin-top': '2px' }}>collapse</button>
            </Show>
          </Show>
        </div>
      </Show>
    }>
      <span style={{ color: '#f87171' }}>⚠ {props.ss.error}</span>
    </Show>
  );
}

// ── Expandable row for action details ────────────────────────────────────

function ActionDetailsCell(props: { details: Record<string, unknown> }) {
  const [expanded, setExpanded] = createSignal(false);
  const full = () => formatJson(props.details);
  const summary = () => summarizeDetails(props.details);

  return (
    <div>
      <span style={{ 'font-family': 'monospace', 'font-size': '0.92em' }}>{summary()}</span>
      <Show when={Object.keys(props.details).length > 0}>
        <button onClick={() => setExpanded(p => !p)} style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'hsl(var(--primary))', 'font-size': '0.85em', 'margin-left': '6px' }}>
          {expanded() ? 'hide' : 'details'}
        </button>
        <Show when={expanded()}>
          <pre style={{ margin: '4px 0 0', 'font-family': 'monospace', 'font-size': '0.85em', 'white-space': 'pre-wrap', 'word-break': 'break-all', background: 'hsl(var(--muted) / 0.3)', 'border-radius': '4px', padding: '8px' }}>{full()}</pre>
        </Show>
      </Show>
    </div>
  );
}

// ── Main Dialog Component ────────────────────────────────────────────────

export default function TestResultDetailsDialog(props: TestResultDetailsDialogProps) {
  const [activeTab, setActiveTab] = createSignal<'steps' | 'output' | 'actions'>('steps');

  const r = () => props.result;
  const stepCount = () => r()?.step_results?.length ?? 0;
  const actionCount = () => r()?.intercepted_actions_total ?? r()?.intercepted_actions?.length ?? 0;

  return (
    <Dialog open={props.open} onOpenChange={(open) => { if (!open) props.onClose(); }}>
      <DialogContent
        class="max-w-[800px] max-h-[85vh] flex flex-col overflow-hidden p-0"
        onInteractOutside={(e: Event) => e.preventDefault()}
      >
        <DialogHeader class="px-6 pt-5 pb-3">
          <DialogTitle class="text-base flex items-center gap-3">
            <Show when={r()}>
              <span>{r()!.passed ? '✅' : '❌'}</span>
              <span>{r()!.test_name}</span>
              <span style={{ 'font-size': '0.8em', 'font-weight': '400', color: statusColor(r()!.actual_status ?? ''), 'margin-left': '4px' }}>
                {r()!.actual_status ?? '—'}
              </span>
              <span style={{ 'font-size': '0.75em', 'font-weight': '400', color: 'hsl(var(--muted-foreground))' }}>
                {r()!.duration_ms}ms
              </span>
            </Show>
          </DialogTitle>
        </DialogHeader>

        <Show when={r()}>
          {(result) => (
            <>
              {/* Assertion failures banner */}
              <Show when={result().failures.length > 0}>
                <div style={{ padding: '0 24px 8px', display: 'flex', 'flex-direction': 'column', gap: '4px' }}>
                  <For each={result().failures}>
                    {(f: WorkflowTestFailure) => (
                      <div style={{ background: 'hsl(0 84% 60% / 0.1)', 'border-radius': '6px', padding: '8px 12px', 'font-size': '0.875em' }}>
                        <div style={{ 'font-weight': '600', color: 'hsl(0 84% 60%)', 'margin-bottom': '2px' }}>{f.expectation}</div>
                        <div style={{ 'font-family': 'monospace', 'font-size': '0.92em' }}>
                          <span style={{ color: 'hsl(var(--muted-foreground))' }}>Expected: </span>{f.expected}
                        </div>
                        <div style={{ 'font-family': 'monospace', 'font-size': '0.92em' }}>
                          <span style={{ color: 'hsl(var(--muted-foreground))' }}>Actual: </span>{f.actual}
                        </div>
                      </div>
                    )}
                  </For>
                </div>
              </Show>

              {/* Tab bar */}
              <div style={{ padding: '0 24px 8px', display: 'flex', gap: '4px' }}>
                <button style={tabBtn(activeTab() === 'steps')} onClick={() => setActiveTab('steps')}>
                  Steps <Show when={stepCount() > 0}><span style={{ opacity: '0.7' }}>({stepCount()})</span></Show>
                </button>
                <button style={tabBtn(activeTab() === 'output')} onClick={() => setActiveTab('output')}>
                  Output
                </button>
                <button style={tabBtn(activeTab() === 'actions')} onClick={() => setActiveTab('actions')}>
                  Actions <Show when={actionCount() > 0}><span style={{ opacity: '0.7' }}>({actionCount()})</span></Show>
                </button>
              </div>

              {/* Tab content */}
              <DialogBody class="px-6 pb-4 overflow-y-auto flex-1">
                {/* Steps tab */}
                <Show when={activeTab() === 'steps'}>
                  <Show when={stepCount() > 0} fallback={
                    <div class="text-muted-foreground text-sm py-4 text-center">No step data available.</div>
                  }>
                    <div style={{ 'overflow-x': 'auto' }}>
                      <table style={{ width: '100%', 'border-collapse': 'collapse' }}>
                        <thead>
                          <tr>
                            <th style={thStyle}>Step</th>
                            <th style={thStyle}>Status</th>
                            <th style={thStyle}>Output / Error</th>
                          </tr>
                        </thead>
                        <tbody>
                          <For each={result().step_results}>
                            {(ss: StepStateSnapshot) => (
                              <tr>
                                <td style={{ ...tdStyle, 'font-family': 'monospace', 'white-space': 'nowrap' }}>{ss.step_id}</td>
                                <td style={{ ...tdStyle, color: statusColor(ss.status), 'font-weight': '600', 'white-space': 'nowrap' }}>{ss.status}</td>
                                <td style={{ ...tdStyle, 'max-width': '400px' }}>
                                  <StepOutputCell ss={ss} />
                                </td>
                              </tr>
                            )}
                          </For>
                        </tbody>
                      </table>
                    </div>
                  </Show>
                </Show>

                {/* Output tab */}
                <Show when={activeTab() === 'output'}>
                  <Show when={result().actual_output !== undefined && result().actual_output !== null} fallback={
                    <div class="text-muted-foreground text-sm py-4 text-center">No output produced.</div>
                  }>
                    <pre style={monoBlock}>{formatJson(result().actual_output)}</pre>
                  </Show>
                </Show>

                {/* Actions tab */}
                <Show when={activeTab() === 'actions'}>
                  <Show when={(result().intercepted_actions?.length ?? 0) > 0} fallback={
                    <div class="text-muted-foreground text-sm py-4 text-center">No intercepted actions — all side effects were mocked.</div>
                  }>
                    <div style={{ 'overflow-x': 'auto' }}>
                      <table style={{ width: '100%', 'border-collapse': 'collapse' }}>
                        <thead>
                          <tr>
                            <th style={thStyle}>Step</th>
                            <th style={thStyle}>Kind</th>
                            <th style={thStyle}>Details</th>
                          </tr>
                        </thead>
                        <tbody>
                          <For each={result().intercepted_actions}>
                            {(a: InterceptedActionSnapshot) => (
                              <tr>
                                <td style={{ ...tdStyle, 'font-family': 'monospace', 'white-space': 'nowrap' }}>{a.step_id}</td>
                                <td style={{ ...tdStyle, 'white-space': 'nowrap' }}>{kindLabel(a.kind)}</td>
                                <td style={{ ...tdStyle, 'max-width': '400px' }}>
                                  <ActionDetailsCell details={a.details} />
                                </td>
                              </tr>
                            )}
                          </For>
                        </tbody>
                      </table>
                    </div>
                    <Show when={(result().intercepted_actions_total ?? 0) > (result().intercepted_actions?.length ?? 0)}>
                      <div class="text-muted-foreground text-xs mt-2">
                        Showing {result().intercepted_actions!.length} of {result().intercepted_actions_total} actions.
                      </div>
                    </Show>
                  </Show>
                </Show>
              </DialogBody>

              <DialogFooter class="px-6 py-3 border-t border-border">
                <Button variant="outline" onClick={props.onClose}>Close</Button>
              </DialogFooter>
            </>
          )}
        </Show>
      </DialogContent>
    </Dialog>
  );
}
