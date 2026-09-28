'use client';
import { useSearchContext } from 'fumadocs-ui/contexts/search';
import { Search } from 'lucide-react';

/** Opens the search dialog (⌘K) from the 404 page. */
export function NotFoundSearch() {
  const { enabled, hotKey, setOpenSearch } = useSearchContext();
  if (!enabled) return null;
  return (
    <button
      type="button"
      onClick={() => setOpenSearch(true)}
      className="inline-flex h-9 w-full max-w-[360px] items-center gap-2 rounded-full border border-border-strong bg-surface-100 ps-3.5 pe-1.5 text-[14px] text-foreground-lighter transition-colors hover:border-border-stronger hover:text-foreground-light focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-(--ring)"
    >
      <Search className="size-4" aria-hidden="true" />
      <span>Search the docs…</span>
      <kbd className="ms-auto inline-flex h-6 items-center gap-0.5 rounded-full border border-border bg-surface-200 px-2 font-sans text-[11px] text-foreground-lighter">
        {hotKey.map((k, i) => (
          <span key={i}>{k.display}</span>
        ))}
      </kbd>
    </button>
  );
}
