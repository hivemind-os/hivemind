import type { Component, ComponentProps, JSX, ValidComponent } from 'solid-js';
import { splitProps } from 'solid-js';

import * as DialogPrimitive from '@kobalte/core/dialog';
import type { PolymorphicProps } from '@kobalte/core/polymorphic';

import { cn } from '~/lib/utils';

const Dialog = DialogPrimitive.Root;
const DialogTrigger = DialogPrimitive.Trigger;

const DialogPortal: Component<DialogPrimitive.DialogPortalProps> = (props) => {
  const [, rest] = splitProps(props, ['children']);
  return (
    <DialogPrimitive.Portal {...rest}>
      <div class="fixed inset-0 z-[1300] flex items-start justify-center sm:items-center pointer-events-none">{props.children}</div>
    </DialogPrimitive.Portal>
  );
};

type DialogOverlayProps<T extends ValidComponent = 'div'> = DialogPrimitive.DialogOverlayProps<T> & {
  class?: string | undefined;
};

const DialogOverlay = <T extends ValidComponent = 'div'>(
  props: PolymorphicProps<T, DialogOverlayProps<T>>,
) => {
  const [, rest] = splitProps(props as DialogOverlayProps, ['class']);
  return (
    <DialogPrimitive.Overlay
      class={cn(
        'fixed inset-0 z-[1300] bg-black/60 pointer-events-auto data-[expanded]:animate-in data-[closed]:animate-out data-[closed]:fade-out-0 data-[expanded]:fade-in-0',
        props.class,
      )}
      {...rest}
    />
  );
};

type DialogContentProps<T extends ValidComponent = 'div'> = DialogPrimitive.DialogContentProps<T> & {
  class?: string | undefined;
  children?: JSX.Element;
};

const DialogContent = <T extends ValidComponent = 'div'>(
  props: PolymorphicProps<T, DialogContentProps<T>>,
) => {
  const [, rest] = splitProps(props as DialogContentProps, ['class', 'children']);
  return (
    <DialogPortal>
      <DialogOverlay />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        class={cn(
          'fixed left-1/2 top-1/2 z-[1301] flex max-h-[calc(100vh-2rem)] w-full max-w-lg -translate-x-1/2 -translate-y-1/2 flex-col gap-4 overflow-y-auto overflow-x-hidden border border-popover-border bg-popover p-6 shadow-xl shadow-black/25 duration-200 pointer-events-auto data-[expanded]:animate-in data-[closed]:animate-out data-[closed]:fade-out-0 data-[expanded]:fade-in-0 data-[closed]:zoom-out-95 data-[expanded]:zoom-in-95 sm:rounded-lg',
          props.class,
        )}
        {...rest}
      >
        {props.children}
      </DialogPrimitive.Content>
    </DialogPortal>
  );
};

const DialogHeader: Component<ComponentProps<'div'>> = (props) => {
  const [, rest] = splitProps(props, ['class']);
  return <div data-slot="dialog-header" class={cn('shrink-0 flex flex-col space-y-1.5 text-center sm:text-left', props.class)} {...rest} />;
};

const DialogBody: Component<ComponentProps<'div'>> = (props) => {
  const [, rest] = splitProps(props, ['class']);
  return <div data-slot="dialog-body" class={cn('min-h-0 flex-1 overflow-y-auto overflow-x-hidden', props.class)} {...rest} />;
};

const DialogFooter: Component<ComponentProps<'div'>> = (props) => {
  const [, rest] = splitProps(props, ['class']);
  return (
    <div
      data-slot="dialog-footer"
      class={cn(
        'sticky bottom-0 z-[1] mt-auto flex shrink-0 flex-col-reverse gap-2 bg-popover sm:flex-row sm:justify-end',
        props.class,
      )}
      {...rest}
    />
  );
};

type DialogTitleProps<T extends ValidComponent = 'h2'> = DialogPrimitive.DialogTitleProps<T> & {
  class?: string | undefined;
};

const DialogTitle = <T extends ValidComponent = 'h2'>(props: PolymorphicProps<T, DialogTitleProps<T>>) => {
  const [, rest] = splitProps(props as DialogTitleProps, ['class']);
  return (
    <DialogPrimitive.Title class={cn('text-lg font-semibold leading-none tracking-tight', props.class)} {...rest} />
  );
};

type DialogDescriptionProps<T extends ValidComponent = 'p'> = DialogPrimitive.DialogDescriptionProps<T> & {
  class?: string | undefined;
};

const DialogDescription = <T extends ValidComponent = 'p'>(
  props: PolymorphicProps<T, DialogDescriptionProps<T>>,
) => {
  const [, rest] = splitProps(props as DialogDescriptionProps, ['class']);
  return <DialogPrimitive.Description class={cn('text-sm text-muted-foreground', props.class)} {...rest} />;
};

export { Dialog, DialogTrigger, DialogContent, DialogHeader, DialogBody, DialogFooter, DialogTitle, DialogDescription };
