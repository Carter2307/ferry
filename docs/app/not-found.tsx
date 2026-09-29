import Link from 'next/link';
import type { Metadata } from 'next';
import { ArrowRight, BookOpen, Rocket, Wrench } from 'lucide-react';
import { SiteHeader } from '@/components/layout/site-header';
import { NotFoundSearch } from '@/components/not-found-search';

export const metadata: Metadata = {
  title: 'Page not found',
};

const LINKS = [
  { icon: <BookOpen />, label: 'Introduction', text: 'What Ferry is and how it works', href: '/docs' },
  { icon: <Rocket />, label: 'Quickstart', text: 'Deploy your first app in a few minutes', href: '/docs/getting-started/quickstart' },
  { icon: <Wrench />, label: 'Troubleshooting', text: 'Common errors and how to fix them', href: '/docs/guides/troubleshooting' },
];

/**
 * The not-found page (exported as out/404.html, which static hosts — including
 * Ferry's nginx static sites — serve for unknown paths).
 */
export default function NotFound() {
  return (
    <>
      <SiteHeader />
      <main className="flex flex-1 flex-col">
        <div className="mx-auto flex w-full max-w-[640px] flex-col px-4 py-16 md:py-24">
          <p className="mono-label mb-3">404 · Not found</p>
          <h1 className="text-[28px] leading-tight font-medium tracking-[-0.015em] text-foreground">
            This page doesn’t exist
          </h1>
          <p className="mt-3 text-[15px] leading-relaxed text-foreground-light">
            The link may be out of date, or the page may have moved. Search the docs, or start from one of these pages.
          </p>
          <div className="mt-8">
            <NotFoundSearch />
          </div>
          <div className="mt-8 flex flex-col gap-3">
            {LINKS.map((l) => (
              <Link
                key={l.href}
                href={l.href}
                className="group flex items-center gap-3 rounded-[var(--ferry-radius-lg)] border border-border bg-surface-100 p-4 transition-colors hover:border-border-stronger"
              >
                <span className="text-foreground-lighter transition-colors group-hover:text-primary [&_svg]:size-[18px]">
                  {l.icon}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-[14px] font-medium text-foreground">{l.label}</span>
                  <span className="block text-[13px] text-foreground-light">{l.text}</span>
                </span>
                <ArrowRight
                  className="size-4 shrink-0 text-foreground-muted transition-transform group-hover:translate-x-0.5 group-hover:text-foreground-light"
                  aria-hidden="true"
                />
              </Link>
            ))}
          </div>
        </div>
      </main>
    </>
  );
}
