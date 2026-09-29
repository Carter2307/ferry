import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';
import { FerryLogo } from '@/components/ferry-logo';
import { SidebarSiteNav } from '@/components/layout/site-nav';
import { githubUrl } from './shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: (
        <>
          <FerryLogo className="size-[18px]" />
          <span className="font-medium">Ferry</span>
        </>
      ),
    },
    githubUrl,
    // The header's section and Dashboard links, at the top of the sidebar and the
    // mobile drawer below `lg` (where the header hides them).
    links: [{ type: 'custom', children: <SidebarSiteNav /> }],
  };
}
