import { Component, For, Show, createSignal, type JSX } from 'solid-js';
import { Sparkles, RefreshCw, HelpCircle, Loader2, Send, Play, GripHorizontal } from 'lucide-solid';

// ── Types ──────────────────────────────────────────────────────────────

export interface AiQuestion {
  request_id: string;
  agent_id: string;
  text: string;
  choices: string[];
  allow_freeform: boolean;
  multi_select?: boolean;
  message?: string;
}

export interface AiAssistPanelProps {
  response: string;
  loading: boolean;
  agent_id: string | null;
  panelHeight: number;
  prompt: string;
  pendingQuestion: AiQuestion | null;
  questionFreeform: string;
  questionSubmitting: boolean;
  onPanelHeightChange: (h: number) => void;
  onPromptChange: (p: string) => void;
  onSend: () => void;
  onClose: () => void;
  onNewConversation: () => void;
  onQuestionFreeformChange: (s: string) => void;
  onAnswerQuestion: (selected_choice?: number, text?: string, selected_choices?: number[]) => void;
  responseRef?: (el: HTMLDivElement) => void;
}

// ── Component ──────────────────────────────────────────────────────────

export function AiAssistPanel(props: AiAssistPanelProps) {
  const [aiMsSelected, setAiMsSelected] = createSignal<Set<number>>(new Set());

  return (
    <div style={{
      'border-top': '1px solid hsl(var(--border))',
      background: 'hsl(var(--card))',
      display: 'flex', 'flex-direction': 'column',
      height: `${props.panelHeight}px`, 'min-height': '120px', 'max-height': '600px',
      position: 'relative',
    }}>
      {/* Resize handle */}
      <div
        style={{
          position: 'absolute', top: '-4px', left: '0', right: '0', height: '8px',
          cursor: 'ns-resize', 'z-index': '10',
          display: 'flex', 'align-items': 'center', 'justify-content': 'center',
        }}
        onMouseDown={(e) => {
          e.preventDefault();
          const startY = e.clientY;
          const startHeight = props.panelHeight;
          const onMove = (me: MouseEvent) => {
            const delta = startY - me.clientY;
            const newH = Math.min(600, Math.max(120, startHeight + delta));
            props.onPanelHeightChange(newH);
          };
          const onUp = () => {
            document.removeEventListener('mousemove', onMove);
            document.removeEventListener('mouseup', onUp);
          };
          document.addEventListener('mousemove', onMove);
          document.addEventListener('mouseup', onUp);
        }}
      >
        <GripHorizontal size={12} style={{ color: 'hsl(var(--muted-foreground))', opacity: '0.5' }} />
      </div>
      <div style={{
        display: 'flex', 'align-items': 'center', 'justify-content': 'space-between',
        padding: '4px 10px',
        'border-bottom': '1px solid hsl(var(--border))',
        'font-size': '0.78em', color: 'hsl(38 92% 50%)', 'font-weight': '600',
      }}>
        <span><Sparkles size={14} /> AI Assist</span>
        <div style={{ display: 'flex', gap: '4px', 'align-items': 'center' }}>
          <Show when={props.agent_id && !props.loading}>
            <button
              style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '0.85em', padding: '2px 6px' }}
              onClick={() => props.onNewConversation()}
              title="New conversation"
            ><RefreshCw size={14} /> New</button>
          </Show>
          <button
            style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '1em', padding: '2px 4px' }}
            onClick={() => props.onClose()}
            title="Close AI Assist"
          >×</button>
        </div>
      </div>
      {/* Response area */}
      <div
        ref={(el: HTMLDivElement) => { props.responseRef?.(el); }}
        style={{
          flex: '1', 'overflow-y': 'auto', padding: '8px 10px',
          'font-size': '0.78em', color: 'hsl(var(--foreground))',
          'white-space': 'pre-wrap', 'word-wrap': 'break-word',
          'font-family': 'inherit',
        }}
      >
        <Show when={props.response}>
          <div>{props.response}</div>
        </Show>
        <Show when={!props.loading && !props.response && !props.pendingQuestion}>
          <div style={{ color: 'hsl(var(--muted-foreground))' }}>
            Describe what you want the workflow to do, and the AI will create or modify it for you.
          </div>
        </Show>
        {/* Inline question UI */}
        <Show when={props.pendingQuestion}>
          <div style={{
            margin: '8px 0', padding: '10px', 'border-radius': '6px',
            background: 'hsl(var(--primary) / 0.06)',
            border: '1px solid hsl(var(--primary) / 0.2)',
          }}>
            <div style={{
              display: 'flex', 'align-items': 'center', gap: '6px',
              'margin-bottom': '8px', 'font-weight': '600', color: 'hsl(var(--primary))',
              'font-size': '0.9em',
            }}>
              <HelpCircle size={14} /> Question
            </div>
            <div style={{ 'margin-bottom': '8px', 'white-space': 'pre-wrap' }}>
              {props.pendingQuestion!.text}
            </div>
            <Show when={props.pendingQuestion!.choices.length > 0}>
              <div style={{ display: 'flex', 'flex-wrap': 'wrap', gap: '6px', 'margin-bottom': '8px' }}>
                <For each={props.pendingQuestion!.choices}>
                  {(choice, idx) => (
                    <button
                      style={{
                        background: props.pendingQuestion!.multi_select && aiMsSelected().has(idx())
                          ? 'hsl(var(--primary))'
                          : 'hsl(var(--background))',
                        border: '1px solid hsl(var(--border))',
                        color: props.pendingQuestion!.multi_select && aiMsSelected().has(idx())
                          ? 'hsl(var(--primary-foreground))'
                          : 'hsl(var(--foreground))',
                        padding: '4px 10px', 'border-radius': '4px',
                        cursor: props.questionSubmitting ? 'not-allowed' : 'pointer',
                        'font-size': '0.85em',
                      }}
                      disabled={props.questionSubmitting}
                      onClick={() => {
                        if (props.pendingQuestion!.multi_select) {
                          setAiMsSelected((prev) => {
                            const next = new Set(prev);
                            if (next.has(idx())) next.delete(idx());
                            else next.add(idx());
                            return next;
                          });
                        } else {
                          props.onAnswerQuestion(idx());
                        }
                      }}
                    >{choice}</button>
                  )}
                </For>
              </div>
              <Show when={props.pendingQuestion!.multi_select}>
                <div style={{ 'margin-bottom': '8px' }}>
                  <button
                    style={{
                      background: 'hsl(var(--primary) / 0.15)', border: '1px solid hsl(var(--primary))',
                      color: 'hsl(var(--primary))', 'border-radius': '4px',
                      padding: '4px 10px', cursor: (aiMsSelected().size === 0 || props.questionSubmitting) ? 'not-allowed' : 'pointer',
                      'font-size': '0.85em',
                    }}
                    disabled={aiMsSelected().size === 0 || props.questionSubmitting}
                    onClick={() => {
                      const indices = [...aiMsSelected()].sort((a, b) => a - b);
                      props.onAnswerQuestion(undefined, undefined, indices);
                    }}
                  >
                    {props.questionSubmitting ? 'Sending…' : 'Submit'}
                  </button>
                </div>
              </Show>
            </Show>
            <Show when={props.pendingQuestion!.allow_freeform !== false}>
              <div style={{ display: 'flex', gap: '6px' }}>
                <input
                  style={{
                    flex: '1', background: 'hsl(var(--background))',
                    border: '1px solid hsl(var(--border))', 'border-radius': '4px',
                    color: 'hsl(var(--foreground))', padding: '4px 8px',
                    'font-size': '0.82em', outline: 'none',
                  }}
                  placeholder="Type your answer…"
                  value={props.questionFreeform}
                  onInput={(e) => props.onQuestionFreeformChange(e.currentTarget.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && props.questionFreeform.trim()) {
                      props.onAnswerQuestion(undefined, props.questionFreeform.trim());
                    }
                  }}
                  disabled={props.questionSubmitting}
                />
                <button
                  style={{
                    background: 'hsl(var(--primary) / 0.15)', border: '1px solid hsl(var(--primary))',
                    color: 'hsl(var(--primary))', 'border-radius': '4px',
                    padding: '4px 10px', cursor: (!props.questionFreeform.trim() || props.questionSubmitting) ? 'not-allowed' : 'pointer',
                    'font-size': '0.82em',
                  }}
                  disabled={!props.questionFreeform.trim() || props.questionSubmitting}
                  onClick={() => props.onAnswerQuestion(undefined, props.questionFreeform.trim())}
                >
                  {props.questionSubmitting ? <Loader2 size={14} class="animate-spin" /> : <Send size={14} />}
                </button>
              </div>
            </Show>
          </div>
        </Show>
      </div>
      {/* Quick follow-up actions — shown after AI submits a workflow */}
      <Show when={!props.loading && !props.pendingQuestion && props.response.includes('✓ Workflow updated')}>
        <div style={{
          display: 'flex', 'flex-wrap': 'wrap', gap: '4px', padding: '6px 10px',
          'border-top': '1px solid hsl(var(--border) / 0.5)',
        }}>
          {[
            { label: '🛡 Add error handling', prompt: 'Add appropriate error handling (on_error with retry/skip) to all task steps that don\'t have it yet.' },
            { label: '👤 Add approval steps', prompt: 'Add human approval feedback_gate steps before any high-stakes actions (sending messages, making API calls, etc.).' },
            { label: '📢 Add notifications', prompt: 'Add notification steps to inform the user about workflow progress and completion.' },
            { label: '💪 Make more robust', prompt: 'Review and improve this workflow: add timeouts to agent steps, error handling on external calls, validate inputs, and add an end_workflow node if missing.' },
          ].map((action) => (
            <button
              style={{
                background: 'hsl(var(--background))',
                border: '1px solid hsl(var(--border))',
                color: 'hsl(var(--muted-foreground))',
                padding: '3px 8px', 'border-radius': '4px',
                cursor: 'pointer', 'font-size': '0.72em',
                'white-space': 'nowrap',
              }}
              onClick={() => { props.onPromptChange(action.prompt); props.onSend(); }}
            >{action.label}</button>
          ))}
        </div>
      </Show>
      {/* Input area */}
      <div style={{
        display: 'flex', gap: '6px', padding: '6px 10px',
        'border-top': '1px solid hsl(var(--border))',
      }}>
        <input
          style={{
            flex: '1', background: 'hsl(var(--background))',
            border: '1px solid hsl(var(--border))', 'border-radius': '4px',
            color: 'hsl(var(--foreground))', padding: '6px 8px',
            'font-size': '0.82em', outline: 'none',
          }}
          placeholder={props.loading ? 'Working...' : 'Describe what you want...'}
          value={props.prompt}
          onInput={(e) => props.onPromptChange(e.currentTarget.value)}
          onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey && !props.loading) props.onSend(); }}
          disabled={props.loading}
        />
        <button
          style={{
            background: props.loading ? 'hsl(var(--muted-foreground) / 0.2)' : 'hsl(38 92% 50% / 0.15)',
            border: `1px solid ${props.loading ? 'hsl(var(--muted-foreground))' : 'hsl(38 92% 50%)'}`,
            color: props.loading ? 'hsl(var(--muted-foreground))' : 'hsl(38 92% 50%)',
            'border-radius': '4px', padding: '6px 12px', cursor: props.loading ? 'not-allowed' : 'pointer',
            'font-size': '0.82em', 'font-weight': '600', 'white-space': 'nowrap',
          }}
          onClick={props.onSend}
          disabled={props.loading || !props.prompt.trim()}
        >
          {props.loading ? <Loader2 size={14} class="animate-spin" /> : <><Play size={14} /> Send</>}
        </button>
      </div>
    </div>
  );
}
