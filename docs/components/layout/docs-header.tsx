'use client';
import { useNotebookLayout } from 'fumadocs-ui/layouts/notebook';
import { Menu } from 'lucide-react';
import type { ComponentProps } from 'react';
import { SiteHeader } from './site-header';

/**
 * Header slot of the docs layout (Fumadocs "notebook" layout, nav mode "top"):
 * the site header spanning the full grid width, plus the mobile sidebar toggle.
 */
export function DocsHeader(_props: ComponentProps<'header'>) {
  const { slots } = useNotebookLayout();
  const Trigger = slots.sidebar.trigger;
  return (
    <SiteHeader
      className="[grid-column:1/-1] [grid-row:1] [contain:inline-size]"
      menu={
        <Trigger
          className="inline-flex size-8 items-center justify-center rounded-full border border-border-strong bg-surface-100 text-foreground-light transition-colors hover:bg-surface-200 hover:text-foreground md:hidden [&_svg]:size-4"
        >
          <Menu />
        </Trigger>
      }
    />
  );
}
