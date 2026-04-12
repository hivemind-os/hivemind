import { Sparkles } from 'lucide-solid';
import { Button } from '~/ui';

export interface WelcomeStepProps {
  onNext: () => void;
}

const WelcomeStep = (props: WelcomeStepProps) => {
  return (
    <div class="flex flex-col items-center justify-center text-center max-w-lg mx-auto animate-in fade-in slide-in-from-bottom-4 duration-500">
      <div class="mb-6 flex h-20 w-20 items-center justify-center rounded-2xl bg-primary/10">
        <Sparkles size={40} class="text-primary" />
      </div>

      <h1 class="text-3xl font-bold tracking-tight text-foreground">
        Welcome to HiveMind OS
      </h1>

      <p class="mt-3 text-base text-muted-foreground leading-relaxed">
        Your private AI assistant that runs locally on your machine.
        Let's get you set up in just a few steps.
      </p>

      <div class="mt-8 flex flex-col gap-3 w-full max-w-xs">
        <Button size="lg" onClick={props.onNext} class="w-full">
          Get Started
        </Button>
        <p class="text-xs text-muted-foreground">
          This takes about 2 minutes
        </p>
      </div>
    </div>
  );
};

export default WelcomeStep;
