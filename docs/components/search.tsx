'use client';
import {
  SearchDialog,
  SearchDialogClose,
  SearchDialogContent,
  SearchDialogHeader,
  SearchDialogIcon,
  SearchDialogInput,
  SearchDialogList,
  SearchDialogOverlay,
  type SharedProps,
} from 'fumadocs-ui/components/dialog/search';
import type { SortedResult } from 'fumadocs-core/search';
import { useDocsSearch, type SearchClient } from 'fumadocs-core/search/client';
import { staticClient } from 'fumadocs-core/search/client/orama-static';
import { useI18n } from 'fumadocs-ui/contexts/i18n';
import { useMemo } from 'react';

/** Results shown in the dialog. */
const LIMIT = 60;

/** Lowercase words without the highlight markup, with a plural "s" dropped ("hooks" → "hook"). */
function words(text: unknown): string[] {
  const plain = typeof text === 'string' ? text.replace(/<\/?mark>/g, '') : '';
  return (plain.toLowerCase().match(/[a-z0-9]+/g) ?? []).map((w) =>
    w.length > 3 && w.endsWith('s') && !w.endsWith('ss') ? w.slice(0, -1) : w,
  );
}

/**
 * How well a page matches as a whole: its title (4) or one of its headings (2)
 * contains every word of the query, +1 when it is exactly the query. Generated
 * API pages lose a point, so the concept or guide page about a topic comes
 * before the endpoints that mention it.
 */
function pageScore(group: SortedResult[], query: string[]): number {
  let score = 0;
  for (const item of group) {
    if (item.type === 'text') continue;
    const itemWords = words(item.content);
    if (!query.every((q) => itemWords.includes(q))) continue;
    score = Math.max(score, (item.type === 'page' ? 4 : 2) + (itemWords.length === query.length ? 1 : 0));
  }
  if (score > 0 && group[0]?.url.startsWith('/docs/reference/api/')) score -= 1;
  return score;
}

/**
 * The static index ranks each result on its own, so a short table cell that
 * repeats the query can put a page that only mentions a topic above the page
 * about it. This client fetches more results, then orders the pages by
 * `pageScore` (keeping the index's order on ties).
 */
function rankedClient(base: SearchClient): SearchClient {
  return {
    deps: base.deps,
    async search(query) {
      const results = await base.search(query);
      const terms = words(query);
      if (terms.length === 0) return results.slice(0, LIMIT);
      const groups: SortedResult[][] = [];
      for (const item of results) {
        if (item.type === 'page' || groups.length === 0) groups.push([]);
        groups[groups.length - 1].push(item);
      }
      return groups
        .map((group, index) => ({ group, index, score: pageScore(group, terms) }))
        .sort((a, b) => b.score - a.score || a.index - b.index)
        .flatMap(({ group }) => group)
        .slice(0, LIMIT);
    },
  };
}

export default function DefaultSearchDialog(props: SharedProps) {
  const { locale } = useI18n(); // (optional) for i18n
  const client = useMemo(() => rankedClient(staticClient({ locale, search: { limit: 400 } })), [locale]);
  const { search, setSearch, query } = useDocsSearch({ client });

  return (
    <SearchDialog search={search} onSearchChange={setSearch} isLoading={query.isLoading} {...props}>
      <SearchDialogOverlay />
      <SearchDialogContent>
        <SearchDialogHeader>
          <SearchDialogIcon />
          <SearchDialogInput />
          <SearchDialogClose />
        </SearchDialogHeader>
        <SearchDialogList items={query.data !== 'empty' ? query.data : null} />
      </SearchDialogContent>
    </SearchDialog>
  );
}
