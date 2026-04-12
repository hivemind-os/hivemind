import type { Component, ComponentProps, ValidComponent } from 'solid-js';
import { splitProps } from 'solid-js';

import * as ContextMenuPrimitive from '@kobalte/core/context-menu';
import type { PolymorphicProps } from '@kobalte/core/polymorphic';

import { cn } from '~/lib/utils';

const ContextMenu = ContextMenuPrimitive.Root;
const ContextMenuTrigger = ContextMenuPrimitive.Trigger;
const ContextMenuPortal = ContextMenuPrimitive.Portal;
const ContextMenuGroup = ContextMenuPrimitive.Group;

type ContextMenuContentProps<T extends ValidComponent = 'div'> =
  ContextMenuPrimitive.ContextMenuContentProps<T> & {
    class?: string | undefined;
  };

const ContextMenuContent = <T extends ValidComponent = 'div'>(
  props: PolymorphicProps<T, ContextMenuContentProps<T>>
) => {
  const [, rest] = splitProps(props as ContextMenuContentProps, ['class']);
  return (
    <ContextMenuPrimitive.Portal>
      <ContextMenuPrimitive.Content
        class={cn(
          'z-50 min-w-32 origin-[var(--kb-menu-content-transform-origin)] animate-content-hide overflow-hidden rounded-md border bg-popover p-1 text-popover-foreground shadow-md data-[expanded]:animate-content-show',
          props.class
        )}
        {...rest}
      />
    </ContextMenuPrimitive.Portal>
  );
};

type ContextMenuItemProps<T extends ValidComponent = 'div'> =
  ContextMenuPrimitive.ContextMenuItemProps<T> & {
    class?: string | undefined;
  };

const ContextMenuItem = <T extends ValidComponent = 'div'>(
  props: PolymorphicProps<T, ContextMenuItemProps<T>>
) => {
  const [, rest] = splitProps(props as ContextMenuItemProps, ['class']);
  return (
    <ContextMenuPrimitive.Item
      class={cn(
        'relative flex cursor-default select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none transition-colors focus:bg-accent focus:text-accent-foreground data-[disabled]:pointer-events-none data-[disabled]:opacity-50',
        props.class
      )}
      {...rest}
    />
  );
};

type ContextMenuSeparatorProps<T extends ValidComponent = 'hr'> =
  ContextMenuPrimitive.ContextMenuSeparatorProps<T> & {
    class?: string | undefined;
  };

const ContextMenuSeparator = <T extends ValidComponent = 'hr'>(
  props: PolymorphicProps<T, ContextMenuSeparatorProps<T>>
) => {
  const [, rest] = splitProps(props as ContextMenuSeparatorProps, ['class']);
  return (
    <ContextMenuPrimitive.Separator
      class={cn('-mx-1 my-1 h-px bg-muted', props.class)}
      {...rest}
    />
  );
};

const ContextMenuShortcut: Component<ComponentProps<'span'>> = (props) => {
  const [, rest] = splitProps(props, ['class']);
  return <span class={cn('ml-auto text-xs tracking-widest opacity-60', props.class)} {...rest} />;
};

export {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuPortal,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuGroup,
  ContextMenuShortcut,
};
