import { createSignal, onMount, onCleanup, Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { Bot } from 'lucide-solid';
import type { ModelRouterSnapshot, Persona, ToolDefinition } from '../types';
import type { PendingQuestion } from './InlineQuestion';
import LaunchBotDialog, { LaunchConfig } from './LaunchBotDialog';
import AgentStage from './AgentStage';
import { Button } from '~/ui';

interface BotsPageProps {
  availableTools: Accessor<string[]>;
  modelRouter: Accessor<ModelRouterSnapshot | null>;
  personas: Accessor<Persona[]>;
  toolDefinitions?: ToolDefinition[];
  onBotQuestion?: (q: PendingQuestion) => void;
  onBotQuestionAnswered?: (request_id: string, answerText: string) => void;
}

export default function BotsPage(props: BotsPageProps) {
  const [showLaunch, setShowLaunch] = createSignal(false);
  const [pendingQuestions, setPendingQuestions] = createSignal<PendingQuestion[]>([]);
  const [answeredQuestions, setAnsweredQuestions] = createSignal(new Map<string, string>());

  let disposed = false;
  onCleanup(() => { disposed = true; });

  onMount(async () => {
    try {
      const pending = await invoke<Array<{
        agent_id: string;
        agent_name: string;
        request_id: string;
        text: string;
        choices: string[];
        allow_freeform: boolean;
        multi_select?: boolean;
        message?: string;
      }>>('list_session_pending_questions', { session_id: '__service__' });
      if (disposed) return;
      for (const q of pending) {
        const pq: PendingQuestion = {
          request_id: q.request_id,
          text: q.text,
          choices: q.choices,
          allow_freeform: q.allow_freeform,
          multi_select: q.multi_select,
          agent_id: q.agent_id,
          message: q.message,
          is_bot: true,
          timestamp: Date.now(),
        };
        addPendingQuestion(pq);
        props.onBotQuestion?.(pq);
      }
    } catch {
      // Endpoint may not exist on older daemons — ignore
    }
  });

  const addPendingQuestion = (q: PendingQuestion) => {
    setPendingQuestions(prev => {
      if (prev.some(p => p.request_id === q.request_id)) return prev;
      return [...prev, q];
    });
  };

  const markQuestionAnswered = (request_id: string, answerText?: string) => {
    setPendingQuestions(prev => prev.filter(q => q.request_id !== request_id));
    if (answerText !== undefined) {
      setAnsweredQuestions(prev => {
        const next = new Map(prev);
        next.set(request_id, answerText);
        return next;
      });
      props.onBotQuestionAnswered?.(request_id, answerText);
    }
  };

  const handleLaunch = async (config: LaunchConfig) => {
    const persona = config.persona_id
      ? props.personas().find(p => p.id === config.persona_id) ?? null
      : null;
    await invoke('launch_bot', {
      config: {
        friendly_name: config.friendlyName,
        description: persona?.description ?? config.description,
        model: persona?.preferred_models?.[0] ?? null,
        preferred_models: persona?.preferred_models ?? null,
        launch_prompt: config.launchPrompt,
        system_prompt: persona?.system_prompt ?? '',
        mode: config.mode,
        timeout_secs: config.timeoutSecs,
        allowed_tools: config.allowed_tools,
        data_class: config.data_class,
        avatar: persona?.avatar ?? config.avatar,
        color: persona ? persona.color ?? null : null,
        role: persona ? { custom: persona.id } : undefined,
        permission_rules: config.permissionRules,
        persona_id: config.persona_id ?? null,
      },
    });
  };

  return (
    <div class="flex h-full flex-col overflow-hidden">
      <div class="flex items-center justify-between border-b border-input px-4 py-2">
        <h2 class="flex items-center gap-1.5 text-base font-semibold"><Bot size={14} /> Bots</h2>
        <Button size="sm" data-testid="launch-bot-btn" aria-label="Launch new bot" onClick={() => setShowLaunch(true)}>
          + Launch Bot
        </Button>
      </div>

      <div class="flex flex-1 flex-col overflow-hidden">
        <AgentStage
          session_id="__service__"
          mode="service"
          modelRouter={props.modelRouter}
          pendingQuestions={pendingQuestions}
          answeredQuestions={answeredQuestions}
          onQuestionAnswered={markQuestionAnswered}
          onAgentQuestion={(agent_id, request_id, text, choices, allow_freeform, message, multi_select) => {
            const q: PendingQuestion = { request_id, text, choices, allow_freeform, multi_select, agent_id, message, is_bot: true, timestamp: Date.now() };
            addPendingQuestion(q);
            props.onBotQuestion?.(q);
          }}
          personas={props.personas}
        />
      </div>

      <LaunchBotDialog
        open={showLaunch}
        onClose={() => setShowLaunch(false)}
        onLaunch={handleLaunch}
        personas={props.personas}
        availableTools={props.availableTools}
        toolDefinitions={props.toolDefinitions}
      />
    </div>
  );
}
