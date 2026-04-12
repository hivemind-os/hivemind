import { Switch, Match, createSignal, type Accessor } from 'solid-js';
import WizardStepper from './WizardStepper';
import WelcomeStep from './WelcomeStep';
import ProvidersStep from './ProvidersStep';
import ConnectorsStep from './ConnectorsStep';
import WebSearchStep from './WebSearchStep';
import ModelsStep from './ModelsStep';
import TourStep from './TourStep';
import type { AppContext, InstalledModel } from '../../types';

export interface SetupWizardProps {
  localModels: Accessor<InstalledModel[]>;
  startDownloadPolling: () => void;
  loadLocalModels: () => Promise<void>;
  context: Accessor<AppContext | null>;
  onComplete: () => Promise<void>;
}

const STEPS = [
  { id: 'welcome', label: 'Welcome' },
  { id: 'providers', label: 'Providers' },
  { id: 'connectors', label: 'Connectors' },
  { id: 'web-search', label: 'Web Search' },
  { id: 'models', label: 'Models' },
  { id: 'tour', label: 'Tour' },
] as const;

const SetupWizard = (props: SetupWizardProps) => {
  const [currentStep, setCurrentStep] = createSignal(0);

  const goNext = () => setCurrentStep((i) => Math.min(i + 1, STEPS.length - 1));
  const goBack = () => setCurrentStep((i) => Math.max(i - 1, 0));
  const goTo = (index: number) => setCurrentStep(index);

  return (
    <div
      class="fixed inset-0 z-[50] flex flex-col bg-background"
      data-testid="setup-wizard"
    >
      {/* Stepper header */}
      <div class="border-b border-border bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/80">
        <WizardStepper
          steps={[...STEPS]}
          currentIndex={currentStep()}
          onStepClick={goTo}
        />
      </div>

      {/* Step content */}
      <div class="flex-1 overflow-y-auto">
        <div class="flex min-h-full items-center justify-center px-6 py-12">
          <Switch>
            <Match when={currentStep() === 0}>
              <WelcomeStep onNext={goNext} />
            </Match>
            <Match when={currentStep() === 1}>
              <ProvidersStep
                context={props.context}
                localModels={props.localModels}
                onNext={goNext}
                onBack={goBack}
              />
            </Match>
            <Match when={currentStep() === 2}>
              <ConnectorsStep
                context={props.context}
                onNext={goNext}
                onBack={goBack}
                onSkip={goNext}
              />
            </Match>
            <Match when={currentStep() === 3}>
              <WebSearchStep
                context={props.context}
                onNext={goNext}
                onBack={goBack}
                onSkip={goNext}
              />
            </Match>
            <Match when={currentStep() === 4}>
              <ModelsStep
                startDownloadPolling={props.startDownloadPolling}
                loadLocalModels={props.loadLocalModels}
                onNext={goNext}
                onBack={goBack}
                onSkip={goNext}
              />
            </Match>
            <Match when={currentStep() === 5}>
              <TourStep
                onFinish={() => void props.onComplete()}
                onBack={goBack}
              />
            </Match>
          </Switch>
        </div>
      </div>
    </div>
  );
};

export default SetupWizard;
