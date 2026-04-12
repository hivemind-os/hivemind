import { Show, For, createEffect } from 'solid-js';
import type { Accessor, JSX } from 'solid-js';
import type { HiveMindConfigData } from '../../types';

export interface CompactionTabProps {
  editConfig: Accessor<HiveMindConfigData | null>;
  updateCompaction: (field: string, value: unknown) => void;
  configLoadingFallback: (label: string) => JSX.Element;
  availableModels: { id: string; label: string }[];
}

const CompactionTab = (props: CompactionTabProps) => {
  const editConfig = props.editConfig;
  const updateCompaction = props.updateCompaction;

  return (
    <div class="settings-section">
      <Show when={editConfig()} fallback={props.configLoadingFallback('compaction settings')}>
          <>
            <h3>Context Compaction</h3>
            <p class="muted">Automatically summarizes older conversation history when it approaches the model's context window limit, preventing token-exceeded errors.</p>
            <div class="settings-form">
              <label>
                <span>Strategy</span>
                <select value={editConfig()!.compaction.strategy} onChange={(e) => updateCompaction('strategy', e.currentTarget.value)}>
                  <option value="summarize-only">Summarize Only</option>
                  <option value="extract-and-summarize">Extract &amp; Summarize</option>
                  <option value="manual">Manual (disabled)</option>
                </select>
              </label>
              <label>
                <span>Trigger threshold</span>
                <input
                  type="number" min="0.1" max="0.99" step="0.05"
                  value={editConfig()!.compaction.trigger_threshold}
                  onChange={(e) => updateCompaction('trigger_threshold', parseFloat(e.currentTarget.value) || 0.75)}
                />
                <span class="muted" style="font-size:0.8em;">Fraction of context window that triggers compaction (e.g. 0.75 = 75%)</span>
              </label>
              <label>
                <span>Keep recent turns</span>
                <input
                  type="number" min="2" max="100"
                  value={editConfig()!.compaction.keep_recent_turns}
                  onChange={(e) => updateCompaction('keep_recent_turns', parseInt(e.currentTarget.value) || 10)}
                />
                <span class="muted" style="font-size:0.8em;">Number of most recent turns to always keep in raw form</span>
              </label>
              <label>
                <span>Summary max tokens</span>
                <input
                  type="number" min="100" max="4000"
                  value={editConfig()!.compaction.summary_max_tokens}
                  onChange={(e) => updateCompaction('summary_max_tokens', parseInt(e.currentTarget.value) || 800)}
                />
                <span class="muted" style="font-size:0.8em;">Target size in tokens for each compaction summary</span>
              </label>
              <label>
                <span>Max summaries in context</span>
                <input
                  type="number" min="1" max="20"
                  value={editConfig()!.compaction.max_summaries_in_context}
                  onChange={(e) => updateCompaction('max_summaries_in_context', parseInt(e.currentTarget.value) || 5)}
                />
                <span class="muted" style="font-size:0.8em;">Oldest summaries are merged into epoch summaries when this is exceeded</span>
              </label>
              <label>
                <span>Extraction model</span>
                {(() => {
                  let selectRef!: HTMLSelectElement;
                  createEffect(() => {
                    const val = editConfig()!.compaction.extraction_model ?? '';
                    const _models = props.availableModels;
                    if (selectRef) {
                      queueMicrotask(() => { selectRef.value = val; });
                    }
                  });
                  return (
                    <select
                      ref={selectRef}
                      onInput={(e) => updateCompaction('extraction_model', e.currentTarget.value || null)}
                    >
                      <option value="">(Default — use conversation model)</option>
                      <For each={props.availableModels}>
                        {(m) => <option value={m.id}>{m.label}</option>}
                      </For>
                    </select>
                  );
                })()}
                <span class="muted" style="font-size:0.8em;">Use a cheaper/faster model for summarization (e.g. gpt-4.1-mini)</span>
              </label>
            </div>
          </>
      </Show>
    </div>
  );
};

export default CompactionTab;
