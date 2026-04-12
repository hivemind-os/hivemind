import { For } from 'solid-js';
import { Check } from 'lucide-solid';

export interface WizardStep {
  id: string;
  label: string;
}

export interface WizardStepperProps {
  steps: WizardStep[];
  currentIndex: number;
  onStepClick?: (index: number) => void;
}

const WizardStepper = (props: WizardStepperProps) => {
  return (
    <nav class="flex items-center justify-center gap-2 px-8 py-4" aria-label="Setup progress">
      <For each={props.steps}>
        {(step, i) => {
          const isCompleted = () => i() < props.currentIndex;
          const isCurrent = () => i() === props.currentIndex;
          const isClickable = () => isCompleted() && !!props.onStepClick;

          return (
            <>
              {i() > 0 && (
                <div
                  class="h-px flex-1 max-w-[60px] transition-colors duration-300"
                  classList={{
                    'bg-primary': isCompleted(),
                    'bg-border': !isCompleted(),
                  }}
                />
              )}
              <button
                type="button"
                class="flex items-center gap-2 rounded-full px-3 py-1.5 text-xs font-medium transition-all duration-300 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                classList={{
                  'bg-primary text-primary-foreground shadow-sm': isCurrent(),
                  'bg-primary/15 text-primary': isCompleted(),
                  'bg-secondary text-muted-foreground': !isCurrent() && !isCompleted(),
                  'cursor-pointer hover:bg-primary/25': isClickable(),
                  'cursor-default': !isClickable(),
                }}
                disabled={!isClickable()}
                onClick={() => isClickable() && props.onStepClick?.(i())}
                aria-current={isCurrent() ? 'step' : undefined}
              >
                {isCompleted() ? (
                  <Check size={14} class="text-primary" />
                ) : (
                  <span
                    class="flex h-5 w-5 items-center justify-center rounded-full text-[10px] font-bold"
                    classList={{
                      'bg-primary-foreground/20': isCurrent(),
                      'bg-muted-foreground/20': !isCurrent(),
                    }}
                  >
                    {i() + 1}
                  </span>
                )}
                <span class="hidden sm:inline">{step.label}</span>
              </button>
            </>
          );
        }}
      </For>
    </nav>
  );
};

export default WizardStepper;
