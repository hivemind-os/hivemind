import { createSignal, For, Show } from 'solid-js';
import { Send } from 'lucide-solid';
import { Button, TextField, TextFieldInput } from '~/ui';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface InteractionResponseFormProps {
  question: string;
  choices?: string[];
  allow_freeform?: boolean;
  multi_select?: boolean;
  onRespond: (response: { selected_choice?: number; selected_choices?: number[]; freeformText?: string }) => void;
  disabled?: boolean;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/**
 * Re-usable "question + choices + freeform input" form extracted from
 * InlineQuestion and InteractionTriageDialog so every interaction surface
 * renders the same UX.
 */
export default function InteractionResponseForm(props: InteractionResponseFormProps) {
  const [freeformText, setFreeformText] = createSignal('');
  const [selectedIndices, setSelectedIndices] = createSignal<Set<number>>(new Set());

  const showFreeform = () => props.allow_freeform !== false || !props.choices?.length;

  const handleChoiceClick = (index: number) => {
    if (props.multi_select) {
      setSelectedIndices((prev) => {
        const next = new Set(prev);
        if (next.has(index)) next.delete(index);
        else next.add(index);
        return next;
      });
    } else {
      props.onRespond({ selected_choice: index });
    }
  };

  const handleMultiSelectSubmit = () => {
    const indices = [...selectedIndices()].sort((a, b) => a - b);
    if (indices.length === 0) return;
    props.onRespond({ selected_choices: indices });
  };

  const handleFreeformSubmit = () => {
    const text = freeformText().trim();
    if (!text) return;
    props.onRespond({ freeformText: text });
    setFreeformText('');
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleFreeformSubmit();
    }
  };

  return (
    <div class="flex flex-col gap-3">
      {/* Question text */}
      <p class="text-sm font-medium text-foreground">{props.question}</p>

      {/* Choice buttons */}
      <Show when={props.choices && props.choices.length > 0}>
        <div class="flex flex-wrap gap-2">
          <For each={props.choices}>
            {(choice, index) => (
              <Button
                variant={props.multi_select && selectedIndices().has(index()) ? 'default' : 'outline'}
                size="sm"
                class="justify-start"
                onClick={() => handleChoiceClick(index())}
                disabled={props.disabled}
              >
                {choice}
              </Button>
            )}
          </For>
        </div>
        <Show when={props.multi_select}>
          <Button
            size="sm"
            disabled={selectedIndices().size === 0 || props.disabled}
            onClick={handleMultiSelectSubmit}
          >
            Submit
          </Button>
        </Show>
      </Show>

      {/* Freeform text input */}
      <Show when={showFreeform()}>
        <div class="flex items-center gap-2">
          <TextField class="flex-1">
            <TextFieldInput
              type="text"
              placeholder="Type your answer…"
              value={freeformText()}
              onInput={(e: InputEvent) => setFreeformText((e.currentTarget as HTMLInputElement).value)}
              onKeyDown={handleKeyDown}
              disabled={props.disabled}
            />
          </TextField>
          <Button
            size="icon"
            onClick={handleFreeformSubmit}
            disabled={!freeformText().trim() || props.disabled}
          >
            {props.disabled ? <span class="animate-pulse">…</span> : <Send size={16} />}
          </Button>
        </div>
      </Show>
    </div>
  );
}
