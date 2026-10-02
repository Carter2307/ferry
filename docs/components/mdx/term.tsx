'use client';
import { Popover } from '@base-ui/react/popover';
import Link from 'fumadocs-core/link';
import { Fragment, type ReactNode } from 'react';
import { glossary, glossaryEntry, type TermId } from '@/lib/glossary';

/** `code` between backticks → <code>. */
function inline(text: string): ReactNode {
  return text.split('`').map((part, i) =>
    i % 2 ? (
      <code key={i} className="rounded-[var(--ferry-radius-sm)] border border-border bg-surface-200 px-1 font-mono text-[0.88em] text-foreground">
        {part}
      </code>
    ) : (
      <Fragment key={i}>{part}</Fragment>
    ),
  );
}

/**
 * A technical name with its definition: hover, focus or tap shows it.
 * Use it the first time a term shows on a page.
 *
 * <Term id="container" />            → "container"
 * <Term id="container">containers</Term>
 */
export function Term({ id, children }: { id: TermId; children?: ReactNode }) {
  const entry = glossaryEntry(id);
  if (!entry) {
    // An unknown id must not break the page: show the words, and tell the author.
    if (process.env.NODE_ENV !== 'production') console.warn(`<Term id="${id}">: not in lib/glossary.ts`);
    return <>{children ?? id}</>;
  }
  return (
    <Popover.Root>
      {/* A <span>, not a <button>: a button cannot break across lines like text, and it lets the period after it wrap alone. */}
      <Popover.Trigger
        nativeButton={false}
        render={<span />}
        openOnHover
        delay={120}
        closeDelay={80}
        className="inline cursor-help rounded-[2px] text-start underline decoration-foreground-muted decoration-dotted underline-offset-[3px] transition-colors hover:text-foreground hover:decoration-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--ring)] data-[popup-open]:text-foreground data-[popup-open]:decoration-primary"
      >
        {children ?? entry.term}
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Positioner side="top" sideOffset={8} collisionPadding={12} className="z-50">
          <Popover.Popup className="max-w-[300px] origin-[var(--transform-origin)] rounded-[var(--ferry-radius-lg)] border border-border-strong bg-surface-300 px-3.5 py-3 text-[13.5px] leading-relaxed text-foreground-light shadow-overlay transition-[opacity,transform] duration-150 data-[ending-style]:scale-95 data-[ending-style]:opacity-0 data-[starting-style]:scale-95 data-[starting-style]:opacity-0">
            <Popover.Title className="mb-1 text-[13.5px] font-medium text-foreground">{entry.term}</Popover.Title>
            <Popover.Description render={<div />}>{inline(entry.definition)}</Popover.Description>
            <Link href={`/docs/glossary#${id}`} className="mt-2 inline-block text-[12.5px] text-primary hover:underline">
              Glossary →
            </Link>
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** Every term of the glossary, in alphabetical order (the /docs/glossary page). */
export function Glossary() {
  const entries = (Object.entries(glossary) as [TermId, (typeof glossary)[TermId]][]).sort((a, b) =>
    a[1].term.localeCompare(b[1].term, 'en', { sensitivity: 'base' }),
  );
  return (
    <dl className="not-prose my-6 divide-y divide-border border-y border-border">
      {entries.map(([id, entry]) => {
        const see = 'see' in entry ? (entry.see as TermId[]) : [];
        return (
          <div key={id} id={id} className="scroll-mt-24 py-3.5 sm:grid sm:grid-cols-[180px_1fr] sm:gap-6">
            <dt className="text-[14px] font-medium text-foreground">{entry.term}</dt>
            <dd className="m-0 mt-1 text-[14px] leading-relaxed text-foreground-light sm:mt-0">
              {inline(entry.definition)}
              {see.length ? (
                <span className="mt-1 block text-[13px] text-foreground-lighter">
                  See also:{' '}
                  {see.map((other, i) => (
                    <Fragment key={other}>
                      {i ? ', ' : ''}
                      <a href={`#${other}`} className="text-foreground-light underline decoration-border-stronger underline-offset-[3px] hover:text-primary">
                        {glossaryEntry(other)?.term ?? other}
                      </a>
                    </Fragment>
                  ))}
                </span>
              ) : null}
            </dd>
          </div>
        );
      })}
    </dl>
  );
}
