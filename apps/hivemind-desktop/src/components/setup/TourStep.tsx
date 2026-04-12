import { For } from 'solid-js';
import { Bot, Wrench, Layers, Plug, Sparkles } from 'lucide-solid';
import { Button } from '~/ui';

export interface TourStepProps {
  onFinish: () => void;
  onBack: () => void;
}

const FEATURES = [
  {
    icon: Bot,
    title: 'Personas & Skills',
    description: 'Switch between specialized AI personas, each with their own capabilities, tools, and knowledge.',
  },
  {
    icon: Wrench,
    title: 'MCP Tool Integrations',
    description: 'Extend HiveMind OS with Model Context Protocol servers — connect to databases, APIs, and development tools.',
  },
  {
    icon: Layers,
    title: 'Multi-Provider Routing',
    description: 'Seamlessly route requests across multiple AI providers based on capability, cost, and priority.',
  },
  {
    icon: Plug,
    title: 'Connectors & Channels',
    description: 'Integrate with email, calendar, chat platforms, and more — HiveMind OS works where you work.',
  },
];

const TourStep = (props: TourStepProps) => {
  return (
    <div class="flex flex-col items-center w-full max-w-2xl mx-auto animate-in fade-in slide-in-from-right-4 duration-400">
      <div class="mb-2 flex h-12 w-12 items-center justify-center rounded-xl bg-primary/10">
        <Sparkles size={24} class="text-primary" />
      </div>
      <h2 class="text-2xl font-bold text-foreground">You're all set!</h2>
      <p class="mt-2 text-sm text-muted-foreground text-center max-w-md">
        Here's a quick look at what HiveMind OS can do for you.
      </p>

      <div class="mt-6 grid grid-cols-1 sm:grid-cols-2 gap-4 w-full">
        <For each={FEATURES}>
          {(feature) => (
            <div class="rounded-xl border bg-card p-4 transition-all hover:shadow-sm hover:border-primary/20">
              <div class="flex items-start gap-3">
                <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
                  <feature.icon size={18} class="text-primary" />
                </div>
                <div>
                  <h3 class="text-sm font-semibold text-foreground">{feature.title}</h3>
                  <p class="mt-1 text-xs text-muted-foreground leading-relaxed">{feature.description}</p>
                </div>
              </div>
            </div>
          )}
        </For>
      </div>

      <div class="mt-8 flex flex-col items-center gap-3">
        <Button size="lg" onClick={props.onFinish} class="px-8">
          <Sparkles size={16} class="mr-2" />
          Start Using HiveMind OS
        </Button>
        <Button variant="ghost" size="sm" onClick={props.onBack}>
          Back
        </Button>
      </div>
    </div>
  );
};

export default TourStep;
