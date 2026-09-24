import { For } from 'solid-js';
import { Bot, Wrench, Layers, Plug, Sparkles, GitBranch, Rocket } from 'lucide-solid';
import { Button } from '~/ui';

export interface TourStepProps {
  onFinish: () => void;
  onBack: () => void;
}

const FEATURES = [
  {
    icon: Bot,
    title: 'Personas',
    description: 'Open Personas in the sidebar. Each persona has its own instructions, tools, and skills.',
  },
  {
    icon: GitBranch,
    title: 'Workflows and bots',
    description: 'Workflows, Bots, and Scheduler are in the sidebar. The gear on Workflows opens saved definitions.',
  },
  {
    icon: Rocket,
    title: 'Flight Deck',
    description: 'The Flight Deck button at the top right shows running agents, workflows, and the knowledge graph.',
  },
  {
    icon: Plug,
    title: 'Connectors',
    description: 'Settings, then Agents & Automation, then Connectors. That covers email, calendar, and chat.',
  },
  {
    icon: Layers,
    title: 'Models',
    description: 'Settings, then AI & Models, is where you add providers and choose how requests are routed.',
  },
  {
    icon: Wrench,
    title: 'MCP tools',
    description: 'Inside a session, the MCP tab connects servers for databases, APIs, and other tools.',
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
