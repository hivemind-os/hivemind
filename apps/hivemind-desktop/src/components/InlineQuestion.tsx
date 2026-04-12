import { createSignal, For, Show } from 'solid-js';
import { CircleQuestionMark, RotateCcw, Send } from 'lucide-solid';
import { Card, CardContent, Badge, Button, TextField, TextFieldInput } from '~/ui';
import { answerQuestion, respondToGate, type PendingInteraction } from '~/lib/interactionRouting';
import { renderMarkdown } from '~/utils';

export interface PendingQuestion {
  request_id: string;
  text: string;
  choices: string[];
  allow_freeform: boolean;
  multi_select?: boolean;
  session_id?: string;
  agent_id?: string;
  agent_name?: string;
  timestamp: number;
  /** The assistant's accompanying message content (text produced alongside the tool call). */
  message?: string;
  /** Present when this question comes from a workflow feedback gate */
  workflow_instance_id?: number;
  workflow_step_id?: string;
  /** True when the question comes from a bot agent (not a session agent) */
  is_bot?: boolean;
}

interface InlineQuestionProps {
  question: PendingQuestion;
  session_id: string;
  onAnswered: (request_id: string, answerText: string) => void;
}

const InlineQuestion = (props: InlineQuestionProps) => {
  const [freeformText, setFreeformText] = createSignal('');
  const [submitting, setSubmitting] = createSignal(false);
  const [selectedIndices, setSelectedIndices] = createSignal<Set<number>>(new Set());

  const [error, setError] = createSignal('');

  const respond = async (selected_choice?: number, text?: string, selected_choices?: number[]) => {
    if (submitting()) return;
    setSubmitting(true);
    setError('');
    let answerLabel: string;
    if (selected_choices && selected_choices.length > 0) {
      answerLabel = selected_choices.map((i) => props.question.choices[i]).join(', ');
    } else {
      answerLabel = text || (selected_choice !== undefined ? props.question.choices[selected_choice] : '');
    }
    try {
      if (props.question.workflow_instance_id != null && props.question.workflow_step_id) {
        // Workflow feedback gate response
        const response: { selected?: string; text?: string } = {};
        if (selected_choice !== undefined) response.selected = props.question.choices[selected_choice];
        if (text) response.text = text;
        await respondToGate(
          {
            request_id: props.question.request_id,
            entity_id: `workflow/${props.question.workflow_instance_id}`,
            source_name: '',
            type: 'workflow_gate',
            instance_id: props.question.workflow_instance_id,
            step_id: props.question.workflow_step_id,
          } as PendingInteraction,
          response,
        );
      } else {
        await answerQuestion(
          {
            request_id: props.question.request_id,
            entity_id: props.question.agent_id ? `agent/${props.question.agent_id}` : `session/${props.session_id}`,
            source_name: props.question.agent_name ?? '',
            type: 'question',
            session_id: props.question.is_bot ? undefined : props.session_id,
            agent_id: props.question.agent_id,
          } as PendingInteraction,
          {
            ...(selected_choice !== undefined ? { selected_choice } : {}),
            ...(selected_choices !== undefined ? { selected_choices } : {}),
            ...(text ? { text } : {}),
          },
        );
      }
      props.onAnswered(props.question.request_id, answerLabel);
    } catch (err) {
      console.error('Failed to respond to question:', err);
      setError(String(err) || 'Failed to send response. Please try again.');
    } finally {
      setSubmitting(false);
    }
  };

  const handleChoiceClick = (index: number) => {
    if (props.question.multi_select) {
      setSelectedIndices((prev) => {
        const next = new Set(prev);
        if (next.has(index)) next.delete(index);
        else next.add(index);
        return next;
      });
    } else {
      void respond(index, props.question.choices[index]);
    }
  };

  const handleMultiSelectSubmit = () => {
    const indices = [...selectedIndices()].sort((a, b) => a - b);
    if (indices.length === 0) return;
    void respond(undefined, undefined, indices);
  };

  const handleFreeformSubmit = () => {
    const text = freeformText().trim();
    if (!text) return;
    void respond(undefined, text);
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleFreeformSubmit();
    }
  };

  return (
    <Card class="border-primary/30 bg-card inline-question-pulse">
      <CardContent class="p-4">
        <div class="mb-2 flex items-center gap-1.5">
          <Badge variant="outline" class="gap-1">
            <CircleQuestionMark size={14} />
            <Show when={props.question.workflow_instance_id}><RotateCcw size={14} /></Show>
            {props.question.agent_name || 'Agent'} asks
          </Badge>
        </div>
        <Show when={props.question.message}>
          <p class="mb-2 text-sm text-muted-foreground">{props.question.message}</p>
        </Show>
        <p class="mb-3 text-sm font-medium text-foreground prose prose-sm dark:prose-invert max-w-none" innerHTML={renderMarkdown(props.question.text)} />

        <Show when={props.question.choices.length > 0}>
          <div class="mb-3 flex flex-wrap gap-2">
            <For each={props.question.choices}>
              {(choice, index) => (
                <Button
                  variant={props.question.multi_select && selectedIndices().has(index()) ? 'default' : 'outline'}
                  size="sm"
                  onClick={() => handleChoiceClick(index())}
                  disabled={submitting()}
                >
                  {choice}
                </Button>
              )}
            </For>
          </div>
          <Show when={props.question.multi_select}>
            <div class="mb-3">
              <Button
                size="sm"
                disabled={selectedIndices().size === 0 || submitting()}
                onClick={handleMultiSelectSubmit}
              >
                Submit
              </Button>
            </div>
          </Show>
        </Show>

        <div class="flex items-center gap-2">
          <TextField class="flex-1">
            <TextFieldInput
              type="text"
              placeholder="Type your answer…"
              value={freeformText()}
              onInput={(e: InputEvent) => setFreeformText((e.currentTarget as HTMLInputElement).value)}
              onKeyDown={handleKeyDown}
              disabled={submitting()}
            />
          </TextField>
          <Button
            size="icon"
            onClick={handleFreeformSubmit}
            disabled={!freeformText().trim() || submitting()}
          >
            {submitting() ? <span class="animate-pulse">…</span> : <Send size={16} />}
          </Button>
        </div>
        <Show when={error()}>
          <p class="mt-2 text-xs text-destructive">{error()}</p>
        </Show>
      </CardContent>
    </Card>
  );
};

/** Read-only card shown after a question has been answered. */
export const AnsweredQuestion = (props: { question: PendingQuestion; answer: string }) => (
  <Card class="border-muted bg-card/50">
    <CardContent class="p-4">
      <div class="mb-2 flex items-center gap-1.5">
        <Badge variant="secondary" class="gap-1">
          ✅ {props.question.agent_name || 'Agent'} asked
        </Badge>
      </div>
      <Show when={props.question.message}>
        <p class="mb-2 text-sm text-muted-foreground">{props.question.message}</p>
      </Show>
      <p class="mb-2 text-sm text-muted-foreground prose prose-sm dark:prose-invert max-w-none" innerHTML={renderMarkdown(props.question.text)} />
      <p class="text-sm">
        <span class="font-medium text-muted-foreground">Your answer:</span>{' '}
        <span class="text-foreground">{props.answer}</span>
      </p>
    </CardContent>
  </Card>
);

export default InlineQuestion;
