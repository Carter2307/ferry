import Link from 'next/link';
import { FerryLogo } from '@/components/ferry-logo';
import { SiteHeader } from '@/components/layout/site-header';
import { githubUrl } from '@/lib/shared';

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <>
      <SiteHeader />
      <main className="flex flex-1 flex-col">{children}</main>
      <footer className="border-t border-border">
        <div className="mx-auto flex w-full max-w-[1120px] flex-col gap-4 px-4 py-8 text-[13px] text-foreground-lighter sm:flex-row sm:items-center sm:justify-between md:px-8">
          <div className="flex items-center gap-2">
            <FerryLogo className="size-4" />
            <span>
              Ferry — a self-hosted Render alternative, written in Rust.{' '}
              <a className="underline decoration-border-stronger underline-offset-2 hover:text-foreground" href={`${githubUrl}/blob/main/LICENSE`} target="_blank" rel="noreferrer noopener">
                MIT licensed
              </a>
              .
            </span>
          </div>
          <nav className="flex flex-wrap gap-x-5 gap-y-2" aria-label="Footer">
            <Link className="hover:text-foreground" href="/docs">
              Docs
            </Link>
            <Link className="hover:text-foreground" href="/docs/reference/api">
              API reference
            </Link>
            <Link className="hover:text-foreground" href="/docs/contributing/development">
              Contributing
            </Link>
            <a className="hover:text-foreground" href={githubUrl} target="_blank" rel="noreferrer noopener">
              GitHub
            </a>
          </nav>
        </div>
      </footer>
    </>
  );
}
