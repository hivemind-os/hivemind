import { type Accessor } from 'solid-js';
import { Button } from '~/ui';
import ConnectorsTab from '../ConnectorsTab';
import type { AppContext } from '../../types';

export interface ConnectorsStepProps {
  context: Accessor<AppContext | null>;
  onNext: () => void;
  onBack: () => void;
  onSkip: () => void;
}

const ConnectorsStep = (props: ConnectorsStepProps) => {
  const daemon_url = () => props.context()?.daemon_url ?? '';

  return (
    <div class="flex flex-col items-center w-full max-w-4xl mx-auto animate-in fade-in slide-in-from-right-4 duration-400">
      <h2 class="text-2xl font-bold text-foreground">Connectors</h2>
      <p class="mt-2 text-sm text-muted-foreground text-center max-w-md">
        Connect your email, calendar, and messaging services. This step is optional — you can always add connectors later in Settings.
      </p>

      <div class="mt-6 w-full">
        <ConnectorsTab daemon_url={daemon_url()} />
      </div>

      <div class="mt-8 flex items-center gap-3">
        <Button variant="ghost" onClick={props.onBack}>
          Back
        </Button>
        <Button variant="secondary" onClick={props.onSkip}>
          Skip
        </Button>
        <Button onClick={props.onNext}>
          Next
        </Button>
      </div>
    </div>
  );
};

export default ConnectorsStep;
