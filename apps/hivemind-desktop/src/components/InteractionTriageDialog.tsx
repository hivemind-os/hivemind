/**
 * InteractionTriageDialog — when clicking a badge on a parent entity
 * that has multiple children with pending interactions, this dialog
 * lists the children grouped by entity, letting the user pick which
 * to address first. If only one child has interactions, it skips
 * triage and opens the interaction dialog directly.
 */
import { createSignal, For, Show } from 'solid-js';
import { HelpCircle, ShieldAlert, MessageSquare, Send } from 'lucide-solid';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { Button, TextField, TextFieldInput } from '~/ui';
import { highlightYaml } from './YamlHighlight';
import { renderMarkdown } from '~/utils';
import {
  answerQuestion,
  respondToApproval,
  respondToGate,
  type PendingInteraction,
} from '~/lib/interactionRouting';

// ── Types ────────────────────────────────────────────────────────────────

interface TriageGroup {
  entity_id: string;
  label: string;
  interactions: PendingInteraction[];
}

interface InteractionTriageDialogProps {
  interactions: PendingInteraction[];
  open: boolean;
  onClose: () => void;
  onAnswered?: (request_id: string, answerText: string) => void;
}

// ── Helpers ──────────────────────────────────────────────────────────────

function groupByEntity(interactions: PendingInteraction[]): TriageGroup[] {
  const map = new Map<string, PendingInteraction[]>();
  for (const i of interactions) {
    const key = i.entity_id;
    if (!map.has(key)) map.set(key, []);
    map.get(key)!.push(i);
  }
  return Array.from(map.entries()).map(([entity_id, items]) => ({
    entity_id,
    label: items[0].source_name || entity_id,
    interactions: items,
  }));
}

function iconForType(type: PendingInteraction['type']) {
  switch (type) {
    case 'question': return <HelpCircle size={14} />;
    case 'tool_approval': return <ShieldAlert size={14} />;
    case 'workflow_gate': return <MessageSquare size={14} />;
  }
}

function labelForType(type: PendingInteraction['type']) {
  switch (type) {
    case 'question': return 'question';
    case 'tool_approval': return 'approval';
    case 'workflow_gate': return 'gate';
  }
}

// ── Component ────────────────────────────────────────────────────────────

export default function InteractionTriageDialog(props: InteractionTriageDialogProps) {
  const [selectedInteraction, setSelectedInteraction] = createSignal<PendingInteraction | null>(null);
  const [sending, setSending] = createSignal(false);
  const [freeformText, setFreeformText] = createSignal('');

  const groups = () => groupByEntity(props.interactions);

  const handleClose = () => {
    setSelectedInteraction(null);
    setSending(false);
    setFreeformText('');
    props.onClose();
  };

  const handleBack = () => {
    setSelectedInteraction(null);
    setSending(false);
    setFreeformText('');
  };

  // Respond to a question with a choice, multi-select choices, or freeform text
  const handleQuestionResponse = async (interaction: PendingInteraction, choiceIdx?: number, text?: string, selected_choices?: number[]) => {
    if (sending()) return;
    setSending(true);
    try {
      await answerQuestion(interaction, {
        ...(choiceIdx !== undefined ? { selected_choice: choiceIdx } : {}),
        ...(selected_choices !== undefined ? { selected_choices } : {}),
        ...(text ? { text } : {}),
      });
      let label: string;
      if (selected_choices && selected_choices.length > 0 && interaction.choices) {
        label = selected_choices.map((i) => interaction.choices![i]).join(', ');
      } else {
        label = text || (choiceIdx !== undefined && interaction.choices ? interaction.choices[choiceIdx] : '');
      }
      props.onAnswered?.(interaction.request_id, label || '');
      handleClose();
    } catch (err) {
      console.error('Failed to answer question:', err);
      setSending(false);
    }
  };

  // Respond to a tool approval
  const handleApprovalResponse = async (interaction: PendingInteraction, approved: boolean, opts?: { allow_agent?: boolean; allow_session?: boolean }) => {
    if (sending()) return;
    setSending(true);
    try {
      await respondToApproval(interaction, { approved, allow_agent: opts?.allow_agent, allow_session: opts?.allow_session });
      const scope = opts?.allow_session ? ' (session)' : opts?.allow_agent ? ' (agent)' : '';
      props.onAnswered?.(interaction.request_id, `${approved ? 'Approved' : 'Denied'}${scope}`);
      handleClose();
    } catch (err) {
      console.error('Failed to respond to approval:', err);
      setSending(false);
    }
  };

  // Respond to a workflow gate
  const handleGateResponse = async (interaction: PendingInteraction, choice?: string, text?: string) => {
    if (sending()) return;
    setSending(true);
    try {
      const response = choice
        ? { selected: choice, text: text || '' }
        : { selected: text || '', text: text || '' };
      await respondToGate(interaction, response);
      props.onAnswered?.(interaction.request_id, choice || text || '');
      handleClose();
    } catch (err) {
      console.error('Failed to respond to gate:', err);
      setSending(false);
    }
  };

  return (
    <Dialog open={props.open} onOpenChange={(open) => { if (!open) handleClose(); }}>
      <DialogContent class="max-w-[520px] w-[90vw] max-h-[80vh] flex flex-col p-0" onInteractOutside={(e: Event) => e.preventDefault()}>
        <Show when={!selectedInteraction()} fallback={
          <InteractionDetailView
            interaction={selectedInteraction()!}
            sending={sending()}
            freeformText={freeformText()}
            setFreeformText={setFreeformText}
            onBack={handleBack}
            onQuestionResponse={handleQuestionResponse}
            onApprovalResponse={handleApprovalResponse}
            onGateResponse={handleGateResponse}
          />
        }>
          {/* Triage list */}
          <DialogHeader class="px-6 pt-6 pb-2">
            <DialogTitle class="text-sm font-semibold text-foreground">
              {props.interactions.length} item{props.interactions.length !== 1 ? 's' : ''} need attention
            </DialogTitle>
          </DialogHeader>

          <div class="flex-1 overflow-y-auto px-6 py-2">
            <For each={groups()}>
              {(group) => (
                <div class="mb-3">
                  <div class="text-xs font-medium text-muted-foreground mb-1.5">{group.label}</div>
                  <For each={group.interactions}>
                    {(interaction) => (
                      <button
                        class="w-full flex items-center gap-2 rounded-md px-3 py-2 text-left text-sm hover:bg-accent/50 transition-colors cursor-pointer border-none bg-transparent text-foreground"
                        onClick={() => setSelectedInteraction(interaction)}
                      >
                        <span class="flex-shrink-0 text-primary">{iconForType(interaction.type)}</span>
                        <span class="flex-1 truncate">
                          {interaction.type === 'question' && (interaction.text || 'Question')}
                          {interaction.type === 'tool_approval' && `Approve ${interaction.tool_id || 'tool'}`}
                          {interaction.type === 'workflow_gate' && (interaction.prompt || 'Feedback needed')}
                        </span>
                        <span class="text-xs text-muted-foreground">{labelForType(interaction.type)}</span>
                      </button>
                    )}
                  </For>
                </div>
              )}
            </For>
          </div>

          <DialogFooter class="px-6 pb-4 pt-2">
            <Button variant="outline" size="sm" onClick={handleClose}>Close</Button>
          </DialogFooter>
        </Show>
      </DialogContent>
    </Dialog>
  );
}

// ── Detail view for a single interaction ─────────────────────────────────

interface InteractionDetailViewProps {
  interaction: PendingInteraction;
  sending: boolean;
  freeformText: string;
  setFreeformText: (text: string) => void;
  onBack: () => void;
  onQuestionResponse: (interaction: PendingInteraction, choiceIdx?: number, text?: string, selected_choices?: number[]) => void;
  onApprovalResponse: (interaction: PendingInteraction, approved: boolean, opts?: { allow_agent?: boolean; allow_session?: boolean }) => void;
  onGateResponse: (interaction: PendingInteraction, choice?: string, text?: string) => void;
}

function InteractionDetailView(props: InteractionDetailViewProps) {
  const [msSelected, setMsSelected] = createSignal<Set<number>>(new Set());

  const isMultiSelect = () => props.interaction.type === 'question' && props.interaction.multi_select === true;

  const handleChoiceClick = (idx: number) => {
    if (isMultiSelect()) {
      setMsSelected((prev) => {
        const next = new Set(prev);
        if (next.has(idx)) next.delete(idx);
        else next.add(idx);
        return next;
      });
    } else {
      props.onQuestionResponse(props.interaction, idx, undefined);
    }
  };

  const handleMultiSelectSubmit = () => {
    const indices = [...msSelected()].sort((a, b) => a - b);
    if (indices.length === 0) return;
    props.onQuestionResponse(props.interaction, undefined, undefined, indices);
  };

  return (
    <>
      <DialogHeader class="flex flex-row items-center gap-3 px-6 pt-6 pb-2">
        <button
          class="text-xs text-muted-foreground hover:text-foreground cursor-pointer border-none bg-transparent p-0"
          onClick={props.onBack}
        >
          ← Back
        </button>
        <span class="text-lg">{iconForType(props.interaction.type)}</span>
        <div>
          <DialogTitle class="text-sm font-semibold text-foreground">
            {props.interaction.type === 'question' && 'Question'}
            {props.interaction.type === 'tool_approval' && 'Tool Approval Required'}
            {props.interaction.type === 'workflow_gate' && 'Feedback Needed'}
          </DialogTitle>
          <Show when={props.interaction.source_name}>
            <div class="text-xs text-muted-foreground">{props.interaction.source_name}</div>
          </Show>
        </div>
      </DialogHeader>

      <div class="flex-1 overflow-y-auto px-6 py-4">
        {/* Question view */}
        <Show when={props.interaction.type === 'question'}>
          <Show when={props.interaction.message}>
            <div class="mb-3 text-sm text-muted-foreground whitespace-pre-wrap">{props.interaction.message}</div>
          </Show>
          <div class="mb-4 text-sm font-medium">{props.interaction.text}</div>

          <Show when={props.interaction.choices && props.interaction.choices.length > 0}>
            <div class="flex flex-col gap-1.5 mb-4">
              <For each={props.interaction.choices}>
                {(choice, idx) => (
                  <Button
                    variant={isMultiSelect() && msSelected().has(idx()) ? 'default' : 'outline'}
                    size="sm"
                    class="justify-start"
                    disabled={props.sending}
                    onClick={() => handleChoiceClick(idx())}
                  >
                    {choice}
                  </Button>
                )}
              </For>
            </div>
            <Show when={isMultiSelect()}>
              <div class="mb-4">
                <Button
                  size="sm"
                  disabled={msSelected().size === 0 || props.sending}
                  onClick={handleMultiSelectSubmit}
                >
                  {props.sending ? 'Sending…' : 'Submit'}
                </Button>
              </div>
            </Show>
          </Show>

          <Show when={props.interaction.allow_freeform !== false}>
            <div class="flex gap-2">
              <TextField class="flex-1">
                <TextFieldInput
                  type="text"
                  placeholder="Type a response…"
                  value={props.freeformText}
                  onInput={(e: InputEvent) => props.setFreeformText((e.target as HTMLInputElement).value)}
                  onKeyDown={(e: KeyboardEvent) => {
                    if (e.key === 'Enter' && props.freeformText.trim()) {
                      props.onQuestionResponse(props.interaction, undefined, props.freeformText.trim());
                    }
                  }}
                />
              </TextField>
              <Button
                size="sm"
                disabled={props.sending || !props.freeformText.trim()}
                onClick={() => props.onQuestionResponse(props.interaction, undefined, props.freeformText.trim())}
              >
                {props.sending ? 'Sending…' : <Send size={14} />}
              </Button>
            </div>
          </Show>
        </Show>

        {/* Approval view */}
        <Show when={props.interaction.type === 'tool_approval'}>
          <div class="mb-2 text-sm">
            <span class="font-mono text-blue-400">{props.interaction.tool_id}</span>
          </div>
          <Show when={props.interaction.reason}>
            <div class="mb-3 text-sm text-muted-foreground">{props.interaction.reason}</div>
          </Show>
          <Show when={props.interaction.input}>
            <pre class="mb-4 max-h-[200px] overflow-auto whitespace-pre-wrap break-all rounded-md bg-black/30 p-2 font-mono text-xs text-muted-foreground" innerHTML={highlightYaml(props.interaction.input!)} />
          </Show>
        </Show>

        {/* Gate view */}
        <Show when={props.interaction.type === 'workflow_gate'}>
          <div class="mb-4 text-sm font-medium prose prose-sm dark:prose-invert max-w-none" innerHTML={renderMarkdown(props.interaction.prompt || '')} />

          <Show when={props.interaction.choices && props.interaction.choices.length > 0}>
            <div class="flex flex-col gap-1.5 mb-4">
              <For each={props.interaction.choices}>
                {(choice) => (
                  <Button
                    variant="outline"
                    size="sm"
                    class="justify-start"
                    disabled={props.sending}
                    onClick={() => props.onGateResponse(props.interaction, choice, props.freeformText.trim() || undefined)}
                  >
                    {choice}
                  </Button>
                )}
              </For>
            </div>
          </Show>

          <Show when={props.interaction.allow_freeform !== false}>
            <div class="flex gap-2">
              <TextField class="flex-1">
                <TextFieldInput
                  type="text"
                  placeholder="Type a response…"
                  value={props.freeformText}
                  onInput={(e: InputEvent) => props.setFreeformText((e.target as HTMLInputElement).value)}
                  onKeyDown={(e: KeyboardEvent) => {
                    if (e.key === 'Enter' && props.freeformText.trim()) {
                      props.onGateResponse(props.interaction, undefined, props.freeformText.trim());
                    }
                  }}
                />
              </TextField>
              <Button
                size="sm"
                disabled={props.sending || !props.freeformText.trim()}
                onClick={() => props.onGateResponse(props.interaction, undefined, props.freeformText.trim())}
              >
                {props.sending ? 'Sending…' : <Send size={14} />}
              </Button>
            </div>
          </Show>
        </Show>
      </div>

      {/* Approval buttons at bottom */}
      <Show when={props.interaction.type === 'tool_approval'}>
        <DialogFooter class="px-6 pb-4 pt-2 flex flex-wrap gap-2">
          <Button
            variant="outline"
            disabled={props.sending}
            onClick={() => props.onApprovalResponse(props.interaction, false)}
          >
            Deny
          </Button>
          <Button
            disabled={props.sending}
            onClick={() => props.onApprovalResponse(props.interaction, true)}
          >
            {props.sending ? 'Sending…' : 'Approve'}
          </Button>
          <Button
            variant="outline"
            disabled={props.sending}
            onClick={() => props.onApprovalResponse(props.interaction, true, { allow_agent: true })}
          >
            Allow for Agent
          </Button>
          <Button
            variant="outline"
            disabled={props.sending}
            onClick={() => props.onApprovalResponse(props.interaction, true, { allow_session: true })}
          >
            Allow for Session
          </Button>
        </DialogFooter>
      </Show>
    </>
  );
}
