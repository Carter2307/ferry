'use client';
import Link from 'next/link';
import { usePathname } from 'next/navigation';
import { Menu } from '@base-ui/react/menu';
import { useSearchContext } from 'fumadocs-ui/contexts/search';
import { ArrowUpRight, Circle, Monitor, Moon, Search, Sun } from 'lucide-react';
import { useTheme } from 'next-themes';
import { useSyncExternalStore, type ReactNode } from 'react';
import { FerryLogo } from '@/components/ferry-logo';
import { cn } from '@/lib/cn';
import { ferryServerUrl, githubUrl } from '@/lib/shared';
import { isSiteNavActive, SITE_NAV } from './site-nav';

const iconButton =
  'inline-flex size-8 shrink-0 items-center justify-center rounded-full border border-border-strong bg-surface-100 text-foreground-light transition-colors hover:border-border-stronger hover:bg-surface-200 hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-(--ring) [&_svg]:size-4';

function GitHubIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor">
      <path d="M12 .3a12 12 0 0 0-3.8 23.38c.6.12.83-.26.83-.57L9 21.07c-3.34.72-4.04-1.61-4.04-1.61-.55-1.39-1.34-1.76-1.34-1.76-1.08-.74.09-.73.09-.73 1.2.09 1.83 1.24 1.83 1.24 1.07 1.83 2.8 1.3 3.49 1 .1-.78.42-1.31.76-1.61-2.67-.3-5.47-1.33-5.47-5.93 0-1.31.47-2.38 1.24-3.22-.14-.3-.54-1.52.1-3.18 0 0 1-.32 3.3 1.23a11.5 11.5 0 0 1 6 0c2.28-1.55 3.29-1.23 3.29-1.23.64 1.66.24 2.88.12 3.18a4.65 4.65 0 0 1 1.23 3.22c0 4.61-2.8 5.63-5.48 5.92.42.36.81 1.1.81 2.22l-.01 3.29c0 .32.21.69.82.57A12 12 0 0 0 12 .3" />
    </svg>
  );
}

const subscribe = () => () => {};

const THEMES = [
  { value: 'light', label: 'Light', icon: Sun },
  { value: 'dark', label: 'Dark', icon: Moon },
  { value: 'system', label: 'System', icon: Monitor },
] as const;

/** Light / Dark / System, like the web client's theme menu (next-themes stores the choice). */
function ThemeMenu() {
  const { theme, setTheme } = useTheme();
  const mounted = useSyncExternalStore(
    subscribe,
    () => true,
    () => false,
  );
  const current = THEMES.find((t) => t.value === (mounted ? theme : 'system')) ?? THEMES[2];
  const Icon = current.icon;
  return (
    <Menu.Root>
      <Menu.Trigger className={iconButton} aria-label={`Theme: ${current.label}`} title="Theme">
        <Icon />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner align="end" sideOffset={6} className="z-50">
          <Menu.Popup className="w-40 origin-(--transform-origin) rounded-lg border border-border-strong bg-fd-popover p-1 text-fd-popover-foreground shadow-overlay outline-none transition-[opacity,scale] duration-150 data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0">
            <Menu.Group>
              <Menu.GroupLabel className="px-2 pt-2 pb-1 font-mono text-[11px] tracking-[0.06em] text-foreground-lighter uppercase">
                Theme
              </Menu.GroupLabel>
              <Menu.RadioGroup value={current.value} onValueChange={(value: string) => setTheme(value)}>
                {THEMES.map((t) => (
                  <Menu.RadioItem
                    key={t.value}
                    value={t.value}
                    closeOnClick
                    className="relative flex cursor-default items-center gap-2 rounded-[5px] py-1.5 pr-2 pl-8 text-[13px] text-foreground-light outline-none select-none data-highlighted:bg-surface-200 data-highlighted:text-foreground [&_svg]:size-4 [&_svg]:shrink-0"
                  >
                    <Menu.RadioItemIndicator className="pointer-events-none absolute left-2 flex size-3.5 items-center justify-center">
                      <Circle className="size-2! fill-current" />
                    </Menu.RadioItemIndicator>
                    <t.icon className="text-foreground-lighter" aria-hidden="true" />
                    {t.label}
                  </Menu.RadioItem>
                ))}
              </Menu.RadioGroup>
            </Menu.Group>
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}

function SearchPill() {
  const { enabled, hotKey, setOpenSearch } = useSearchContext();
  if (!enabled) return null;
  return (
    <>
      <button
        type="button"
        onClick={() => setOpenSearch(true)}
        className="hidden h-8 w-52 items-center gap-2 rounded-full border border-border-strong bg-surface-100 ps-3 pe-1.5 text-[13px] text-foreground-lighter transition-colors hover:border-border-stronger hover:text-foreground-light md:inline-flex lg:w-60"
        aria-label="Search the docs"
      >
        <Search className="size-3.5" aria-hidden="true" />
        <span>Search docs…</span>
        <kbd className="ms-auto inline-flex h-5 items-center gap-0.5 rounded-full border border-border bg-surface-200 px-2 font-sans text-[11px] text-foreground-lighter">
          {hotKey.map((k, i) => (
            <span key={i}>{k.display}</span>
          ))}
        </kbd>
      </button>
      <button type="button" onClick={() => setOpenSearch(true)} className={cn(iconButton, 'md:hidden')} aria-label="Search the docs">
        <Search />
      </button>
    </>
  );
}

/**
 * The 48px top bar shared by the landing page and the docs: logo + DOCS pill,
 * section links, search pill (⌘K), GitHub, Dashboard and the theme menu.
 * Below `lg` the section links (and below `sm` the Dashboard link) move to the
 * top of the sidebar drawer: see SidebarSiteNav.
 * `menu` is the docs' mobile sidebar trigger.
 */
export function SiteHeader({ className, menu }: { className?: string; menu?: ReactNode }) {
  const pathname = usePathname();
  return (
    <header
      id="ferry-header"
      className={cn(
        'sticky top-0 z-40 flex h-12 items-center gap-3 border-b border-border bg-background/85 px-4 backdrop-blur-md supports-[backdrop-filter]:bg-background/75 md:px-6',
        className,
      )}
    >
      <Link href="/" className="flex shrink-0 items-center gap-2 rounded-[var(--ferry-radius-md)]" aria-label="Ferry Docs home">
        <FerryLogo className="size-[18px]" />
        <span className="text-[15px] font-medium tracking-[-0.01em] text-foreground">Ferry</span>
      </Link>
      <Link
        href="/docs"
        className="inline-flex h-5 items-center rounded-full border border-border-strong px-2 font-mono text-[10.5px] tracking-[0.06em] text-foreground-light uppercase transition-colors hover:border-border-stronger hover:text-foreground"
      >
        Docs
      </Link>

      <nav className="ms-4 hidden items-center gap-5 lg:flex" aria-label="Sections">
        {SITE_NAV.map((item) => {
          const active = isSiteNavActive(item, pathname);
          return (
            <Link
              key={item.href}
              href={item.href}
              data-active={active}
              className="text-[14px] text-foreground-light transition-colors hover:text-foreground data-[active=true]:text-foreground data-[active=true]:font-medium"
            >
              {item.label}
            </Link>
          );
        })}
      </nav>

      <div className="ms-auto flex items-center gap-2">
        <SearchPill />
        <a href={githubUrl} target="_blank" rel="noreferrer noopener" className={iconButton} aria-label="Ferry on GitHub">
          <GitHubIcon />
        </a>
        <a
          href={ferryServerUrl}
          target="_blank"
          rel="noreferrer noopener"
          title={`Open your Ferry dashboard (${ferryServerUrl})`}
          className="hidden h-8 items-center gap-1 rounded-full border border-border-strong bg-surface-100 px-3 text-[13px] font-medium text-foreground transition-colors hover:border-border-stronger hover:bg-surface-200 sm:inline-flex"
        >
          Dashboard
          <ArrowUpRight className="size-3.5 text-foreground-lighter" aria-hidden="true" />
        </a>
        <ThemeMenu />
        {menu}
      </div>
    </header>
  );
}
