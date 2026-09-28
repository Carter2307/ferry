import type { CSSProperties } from 'react';
import { DocsLayout } from 'fumadocs-ui/layouts/notebook';
import { DocsHeader } from '@/components/layout/docs-header';
import { sidebarComponents } from '@/components/layout/sidebar';
import { baseOptions } from '@/lib/layout.shared';
import { source } from '@/lib/source';

export default function Layout({ children }: LayoutProps<'/docs'>) {
  return (
    <DocsLayout
      tree={source.getPageTree()}
      {...baseOptions()}
      nav={{ ...baseOptions().nav, mode: 'top' }}
      tabs={false}
      themeSwitch={{ enabled: false }}
      slots={{ header: DocsHeader }}
      sidebar={{ collapsible: false, components: sidebarComponents }}
      containerProps={{ style: { '--fd-header-height': '48px' } as CSSProperties }}
    >
      {children}
    </DocsLayout>
  );
}
