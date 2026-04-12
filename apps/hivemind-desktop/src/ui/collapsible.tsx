import type { ValidComponent } from 'solid-js';
import { splitProps } from 'solid-js';

import * as CollapsiblePrimitive from '@kobalte/core/collapsible';
import type { PolymorphicProps } from '@kobalte/core/polymorphic';

import { cn } from '~/lib/utils';

const Collapsible = CollapsiblePrimitive.Root;
const CollapsibleTrigger = CollapsiblePrimitive.Trigger;

type CollapsibleContentProps<T extends ValidComponent = 'div'> =
  CollapsiblePrimitive.CollapsibleContentProps<T> & {
    class?: string | undefined;
  };

const CollapsibleContent = <T extends ValidComponent = 'div'>(
  props: PolymorphicProps<T, CollapsibleContentProps<T>>,
) => {
  const [local, others] = splitProps(props as CollapsibleContentProps, ['class']);
  return (
    <CollapsiblePrimitive.Content
      class={cn(
        'animate-collapsible-up data-[expanded]:animate-collapsible-down overflow-hidden',
        local.class,
      )}
      {...others}
    />
  );
};

export { Collapsible, CollapsibleTrigger, CollapsibleContent };
