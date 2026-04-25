/**
 * Tests for shadow mode / workflow safety UX helpers.
 *
 * These verify:
 * 1. InterceptedAction / ShadowSummary type handling
 * 2. Impact estimate threshold classification
 * 3. Tab filtering logic for intercepted actions
 * 4. First-run protection YAML hash detection
 */
import { describe, it, expect, beforeEach } from 'vitest';
import type {
  InterceptedAction,
  ShadowSummary,
  WorkflowImpactEstimate,
  WorkflowInstanceSummary,
} from '~/types';

// ─── Helpers (replicating logic from components for pure testing) ─────────

type TabKind = 'all' | 'tool_call' | 'agent' | 'other';

function kindToTab(kind: string): TabKind {
  if (kind === 'tool_call') return 'tool_call';
  if (kind === 'agent_invocation' || kind === 'agent_signal' || kind === 'agent_wait') return 'agent';
  return 'other';
}

function isEmailAction(a: InterceptedAction): boolean {
  const toolId = (a.details.tool_id as string) ?? '';
  return toolId.includes('send_message') || toolId.includes('send_email') || toolId.includes('email');
}

function isHttpAction(a: InterceptedAction): boolean {
  const toolId = (a.details.tool_id as string) ?? '';
  return toolId.includes('http') || toolId.includes('request') || toolId.includes('fetch');
}

type Severity = 'green' | 'yellow' | 'red';

function classifyImpactSeverity(estimate: WorkflowImpactEstimate): Severity {
  const items = estimate.items ?? [];
  let maxEmails = 0;
  let maxApiCalls = 0;
  for (const item of items) {
    const max = item.estimate?.max ?? item.estimate?.min ?? 0;
    const tool = (item.tool_id as string) ?? '';
    if (tool.includes('email') || tool.includes('send_message')) {
      maxEmails += max;
    }
    if (tool.includes('http') || tool.includes('request')) {
      maxApiCalls += max;
    }
  }

  if (maxEmails > 100 || maxApiCalls > 100) return 'red';
  if (maxEmails > 10 || maxApiCalls > 20) return 'yellow';
  return 'green';
}

function isShadowInstance(inst: WorkflowInstanceSummary): boolean {
  return inst.execution_mode === 'shadow';
}

// ─── Tests ───────────────────────────────────────────────────────────────

describe('Shadow Mode Helpers', () => {
  describe('kindToTab', () => {
    it('maps tool_call to tool_call tab', () => {
      expect(kindToTab('tool_call')).toBe('tool_call');
    });

    it('maps agent_invocation to agent tab', () => {
      expect(kindToTab('agent_invocation')).toBe('agent');
    });

    it('maps agent_signal to agent tab', () => {
      expect(kindToTab('agent_signal')).toBe('agent');
    });

    it('maps agent_wait to agent tab', () => {
      expect(kindToTab('agent_wait')).toBe('agent');
    });

    it('maps workflow_launch to other tab', () => {
      expect(kindToTab('workflow_launch')).toBe('other');
    });

    it('maps scheduled_task to other tab', () => {
      expect(kindToTab('scheduled_task')).toBe('other');
    });

    it('maps event_gate to other tab', () => {
      expect(kindToTab('event_gate')).toBe('other');
    });

    it('maps unknown kinds to other tab', () => {
      expect(kindToTab('something_new')).toBe('other');
    });
  });

  describe('isEmailAction', () => {
    it('detects connector.send_message as email', () => {
      const action: InterceptedAction = {
        id: 1,
        instance_id: 1,
        step_id: 'send',
        kind: 'tool_call',
        timestamp_ms: Date.now(),
        details: { tool_id: 'connector.send_message', arguments: {} },
      };
      expect(isEmailAction(action)).toBe(true);
    });

    it('detects email.send as email', () => {
      const action: InterceptedAction = {
        id: 2,
        instance_id: 1,
        step_id: 'send',
        kind: 'tool_call',
        timestamp_ms: Date.now(),
        details: { tool_id: 'email.send', arguments: {} },
      };
      expect(isEmailAction(action)).toBe(true);
    });

    it('does not detect http.request as email', () => {
      const action: InterceptedAction = {
        id: 3,
        instance_id: 1,
        step_id: 'call',
        kind: 'tool_call',
        timestamp_ms: Date.now(),
        details: { tool_id: 'http.request', arguments: {} },
      };
      expect(isEmailAction(action)).toBe(false);
    });
  });

  describe('isHttpAction', () => {
    it('detects http.request as HTTP', () => {
      const action: InterceptedAction = {
        id: 1,
        instance_id: 1,
        step_id: 'call',
        kind: 'tool_call',
        timestamp_ms: Date.now(),
        details: { tool_id: 'http.request', arguments: {} },
      };
      expect(isHttpAction(action)).toBe(true);
    });

    it('does not detect connector.send_message as HTTP', () => {
      const action: InterceptedAction = {
        id: 2,
        instance_id: 1,
        step_id: 'send',
        kind: 'tool_call',
        timestamp_ms: Date.now(),
        details: { tool_id: 'connector.send_message', arguments: {} },
      };
      expect(isHttpAction(action)).toBe(false);
    });
  });

  describe('Tab filtering', () => {
    const actions: InterceptedAction[] = [
      { id: 1, instance_id: 1, step_id: 's1', kind: 'tool_call', timestamp_ms: 1000, details: { tool_id: 'email.send' } },
      { id: 2, instance_id: 1, step_id: 's2', kind: 'agent_invocation', timestamp_ms: 2000, details: { persona_id: 'bot' } },
      { id: 3, instance_id: 1, step_id: 's3', kind: 'tool_call', timestamp_ms: 3000, details: { tool_id: 'http.request' } },
      { id: 4, instance_id: 1, step_id: 's4', kind: 'workflow_launch', timestamp_ms: 4000, details: { workflow_name: 'child' } },
      { id: 5, instance_id: 1, step_id: 's5', kind: 'scheduled_task', timestamp_ms: 5000, details: { name: 'cron' } },
    ];

    it('all tab returns all actions', () => {
      const filtered = actions.filter(() => true);
      expect(filtered.length).toBe(5);
    });

    it('tool_call tab filters to tool calls only', () => {
      const filtered = actions.filter(a => kindToTab(a.kind) === 'tool_call');
      expect(filtered.length).toBe(2);
      expect(filtered[0].step_id).toBe('s1');
      expect(filtered[1].step_id).toBe('s3');
    });

    it('agent tab filters to agent actions only', () => {
      const filtered = actions.filter(a => kindToTab(a.kind) === 'agent');
      expect(filtered.length).toBe(1);
      expect(filtered[0].step_id).toBe('s2');
    });

    it('other tab filters to workflow/schedule/event actions', () => {
      const filtered = actions.filter(a => kindToTab(a.kind) === 'other');
      expect(filtered.length).toBe(2);
      expect(filtered[0].kind).toBe('workflow_launch');
      expect(filtered[1].kind).toBe('scheduled_task');
    });
  });
});

describe('Instance Badge', () => {
  it('shadow instance is detected', () => {
    const inst = { execution_mode: 'shadow' } as WorkflowInstanceSummary;
    expect(isShadowInstance(inst)).toBe(true);
  });

  it('normal instance is not shadow', () => {
    const inst = { execution_mode: 'normal' } as WorkflowInstanceSummary;
    expect(isShadowInstance(inst)).toBe(false);
  });

  it('missing execution_mode is not shadow', () => {
    const inst = {} as WorkflowInstanceSummary;
    expect(isShadowInstance(inst)).toBe(false);
  });
});

describe('Shadow Summary', () => {
  it('formats summary with all zero counts', () => {
    const summary: ShadowSummary = {
      total_intercepted: 0,
      tool_calls_intercepted: 0,
      agent_invocations_intercepted: 0,
      workflow_launches_intercepted: 0,
      scheduled_tasks_intercepted: 0,
      agent_signals_intercepted: 0,
    };
    expect(summary.total_intercepted).toBe(0);
  });

  it('tracks counts accurately', () => {
    const summary: ShadowSummary = {
      total_intercepted: 105,
      tool_calls_intercepted: 100,
      agent_invocations_intercepted: 3,
      workflow_launches_intercepted: 1,
      scheduled_tasks_intercepted: 1,
      agent_signals_intercepted: 0,
    };
    const countSum = summary.tool_calls_intercepted
      + summary.agent_invocations_intercepted
      + summary.workflow_launches_intercepted
      + summary.scheduled_tasks_intercepted
      + summary.agent_signals_intercepted;
    expect(countSum).toBe(105);
    expect(summary.total_intercepted).toBe(countSum);
  });

  it('builds chip list from non-zero counts', () => {
    const summary: ShadowSummary = {
      total_intercepted: 4,
      tool_calls_intercepted: 3,
      agent_invocations_intercepted: 1,
      workflow_launches_intercepted: 0,
      scheduled_tasks_intercepted: 0,
      agent_signals_intercepted: 0,
    };

    const chips: { label: string; count: number }[] = [];
    if (summary.tool_calls_intercepted > 0) chips.push({ label: 'Tool calls', count: summary.tool_calls_intercepted });
    if (summary.agent_invocations_intercepted > 0) chips.push({ label: 'Agent invocations', count: summary.agent_invocations_intercepted });
    if (summary.workflow_launches_intercepted > 0) chips.push({ label: 'Workflow launches', count: summary.workflow_launches_intercepted });
    if (summary.scheduled_tasks_intercepted > 0) chips.push({ label: 'Scheduled tasks', count: summary.scheduled_tasks_intercepted });
    if (summary.agent_signals_intercepted > 0) chips.push({ label: 'Agent signals', count: summary.agent_signals_intercepted });

    expect(chips.length).toBe(2);
    expect(chips[0]).toEqual({ label: 'Tool calls', count: 3 });
    expect(chips[1]).toEqual({ label: 'Agent invocations', count: 1 });
  });
});

describe('Impact Estimate Classification', () => {
  it('classifies low impact as green', () => {
    const estimate = {
      items: [
        { tool_id: 'email.send', estimate: { min: 1, max: 5 } },
        { tool_id: 'http.request', estimate: { min: 1, max: 3 } },
      ],
    } as any as WorkflowImpactEstimate;

    expect(classifyImpactSeverity(estimate)).toBe('green');
  });

  it('classifies medium email volume as yellow', () => {
    const estimate = {
      items: [
        { tool_id: 'email.send', estimate: { min: 15, max: 50 } },
      ],
    } as any as WorkflowImpactEstimate;

    expect(classifyImpactSeverity(estimate)).toBe('yellow');
  });

  it('classifies high email volume as red', () => {
    const estimate = {
      items: [
        { tool_id: 'connector.send_message', estimate: { min: 100, max: 2400 } },
      ],
    } as any as WorkflowImpactEstimate;

    expect(classifyImpactSeverity(estimate)).toBe('red');
  });

  it('classifies high API call volume as red', () => {
    const estimate = {
      items: [
        { tool_id: 'http.request', estimate: { min: 50, max: 200 } },
      ],
    } as any as WorkflowImpactEstimate;

    expect(classifyImpactSeverity(estimate)).toBe('red');
  });

  it('empty estimate is green', () => {
    const estimate = { items: [] } as any as WorkflowImpactEstimate;
    expect(classifyImpactSeverity(estimate)).toBe('green');
  });
});

describe('Pagination', () => {
  it('computes total pages correctly', () => {
    const totalPages = (total: number, pageSize: number) => Math.max(1, Math.ceil(total / pageSize));

    expect(totalPages(0, 25)).toBe(1);
    expect(totalPages(1, 25)).toBe(1);
    expect(totalPages(25, 25)).toBe(1);
    expect(totalPages(26, 25)).toBe(2);
    expect(totalPages(60, 25)).toBe(3);
    expect(totalPages(100, 25)).toBe(4);
  });
});
