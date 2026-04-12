import type { JSX } from 'solid-js';
import { cn } from '~/lib/utils';

export interface EmptyStateProps {
  icon?: JSX.Element;
  title: string;
  description?: string;
  children?: JSX.Element;
  compact?: boolean;
}

export function EmptyState(props: EmptyStateProps) {
  return (
    <div class={cn('text-center text-muted-foreground', props.compact ? 'py-6' : 'py-12')}>
      {props.icon && <div class="mb-3 text-4xl">{props.icon}</div>}
      <p class="mb-1 text-base font-medium text-foreground/70">{props.title}</p>
      {props.description && <p class="mb-5 text-sm">{props.description}</p>}
      {props.children}
    </div>
  );
}
