'use client';
import {
  SidebarFolder,
  SidebarFolderContent,
  SidebarFolderLink,
  SidebarFolderTrigger,
  SidebarItem,
} from 'fumadocs-ui/components/sidebar/base';
import type { SidebarPageTreeComponents } from 'fumadocs-ui/components/sidebar/page-tree';
import { usePathname } from 'fumadocs-core/framework';
import type * as PageTree from 'fumadocs-core/page-tree';
import { ArrowUpRight } from 'lucide-react';
import { createContext, use, type ReactNode } from 'react';
import { cn } from '@/lib/cn';

/**
 * Supabase-docs sidebar: every top-level folder of content/docs is a section
 * with an UPPERCASE mono label (its meta.json title) and flat 30px items with a
 * 6px-radius selection background. Nested folders (e.g. Reference → API
 * reference) are collapsible groups.
 */

const DepthContext = createContext(0);

function normalize(url: string) {
  return url.length > 1 && url.endsWith('/') ? url.slice(0, -1) : url;
}

function isActive(url: string, pathname: string) {
  return normalize(url) === normalize(pathname);
}

function containsActive(node: PageTree.Node, pathname: string): boolean {
  if (node.type === 'page') return isActive(node.url, pathname);
  if (node.type === 'folder') {
    return (node.index ? isActive(node.index.url, pathname) : false) || node.children.some((c) => containsActive(c, pathname));
  }
  return false;
}

const itemClass =
  'relative flex min-h-[30px] w-full items-center gap-2 rounded-[var(--ferry-radius-md)] px-2 py-1 text-start text-[14px] leading-5 text-foreground-light transition-colors hover:bg-selection hover:text-foreground data-[active=true]:bg-selection data-[active=true]:font-medium data-[active=true]:text-foreground [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-foreground-lighter';

function Item({ item }: { item: PageTree.Item }) {
  const pathname = usePathname();
  // Items show no icons (Supabase style); external links get an arrow.
  return (
    <SidebarItem
      href={item.url}
      external={item.external}
      active={isActive(item.url, pathname)}
      icon={<></>}
      className={itemClass}
    >
      {item.name}
      {item.external ? <ArrowUpRight className="ms-auto" aria-hidden="true" /> : null}
    </SidebarItem>
  );
}

function Folder({ item, children }: { item: PageTree.Folder; children: ReactNode }) {
  const depth = use(DepthContext);
  const pathname = usePathname();

  if (depth === 0) {
    // A top-level section: mono label + flat list (index page first, if any).
    return (
      <section className="mt-6 first:mt-0" aria-label={typeof item.name === 'string' ? item.name : undefined}>
        <p className="mono-label mb-1.5 flex items-center gap-2 px-2 [&_svg]:size-3.5">
          {item.icon}
          {item.name}
        </p>
        <DepthContext value={1}>
          <div className="flex flex-col gap-px">
            {item.index ? <Item item={item.index} /> : null}
            {children}
          </div>
        </DepthContext>
      </section>
    );
  }

  // A nested group: collapsible, opened when it contains the current page.
  const active = containsActive(item, pathname);
  return (
    <SidebarFolder collapsible={item.collapsible ?? true} defaultOpen={item.defaultOpen} active={active}>
      {item.index ? (
        <SidebarFolderLink href={item.index.url} active={isActive(item.index.url, pathname)} className={itemClass}>
          {item.name}
        </SidebarFolderLink>
      ) : (
        <SidebarFolderTrigger className={itemClass}>{item.name}</SidebarFolderTrigger>
      )}
      <SidebarFolderContent>
        <DepthContext value={depth + 1}>
          <div className="relative ms-3 flex flex-col gap-px border-s ps-2 pt-px">{children}</div>
        </DepthContext>
      </SidebarFolderContent>
    </SidebarFolder>
  );
}

function Separator({ item }: { item: PageTree.Separator }) {
  return (
    <p className="mono-label mt-6 mb-1.5 flex items-center gap-2 px-2 first:mt-0 [&_svg]:size-3.5">
      {item.icon}
      {item.name}
    </p>
  );
}

export const sidebarComponents: Partial<SidebarPageTreeComponents> = { Item, Folder, Separator };

export function sidebarItemClass(className?: string) {
  return cn(itemClass, className);
}
