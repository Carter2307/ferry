'use client';
import { SidebarItem } from 'fumadocs-ui/components/sidebar/base';
import { usePathname } from 'fumadocs-core/framework';
import { ArrowUpRight } from 'lucide-react';
import { ferryServerUrl } from '@/lib/shared';
import { sidebarItemClass } from './sidebar';

/** The top-level sections, linked from the header (desktop) and the sidebar drawer (phones, tablets). */
export const SITE_NAV = [
  { href: '/docs/getting-started/quickstart', match: '/docs/getting-started', label: 'Get started' },
  { href: '/docs/guides/github-auto-deploy', match: '/docs/guides', label: 'Guides' },
  { href: '/docs/reference/cli', match: '/docs/reference', label: 'Reference' },
  { href: '/docs/reference/api', match: '/docs/reference/api', label: 'API' },
];

export function isSiteNavActive(item: (typeof SITE_NAV)[number], pathname: string) {
  // "Reference" is not active on the API pages: "API" is.
  return pathname.startsWith(item.match) && !(item.match === '/docs/reference' && pathname.startsWith('/docs/reference/api'));
}

/**
 * The header's section links and the Dashboard link, for widths where the header
 * hides them (below `lg`): rendered at the top of the sidebar and of the mobile drawer.
 */
export function SidebarSiteNav() {
  const pathname = usePathname();
  return (
    <nav aria-label="Sections" className="flex flex-col gap-px border-b border-border pb-4">
      {SITE_NAV.map((item) => {
        const active = isSiteNavActive(item, pathname);
        return (
          <SidebarItem
            key={item.href}
            href={item.href}
            icon={<></>}
            aria-current={active ? 'true' : undefined}
            className={sidebarItemClass(active ? 'font-medium text-foreground' : undefined)}
          >
            {item.label}
          </SidebarItem>
        );
      })}
      <SidebarItem href={ferryServerUrl} external icon={<></>} className={sidebarItemClass()}>
        Dashboard
        <ArrowUpRight className="ms-auto" aria-hidden="true" />
      </SidebarItem>
    </nav>
  );
}
