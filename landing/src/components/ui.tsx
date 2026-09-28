import type { ReactNode } from 'react'

/** A page section: full-width hairline on top, content in the page container. */
export function Section({
  id,
  labelledBy,
  className = '',
  children,
}: {
  id?: string
  labelledBy: string
  className?: string
  children: ReactNode
}) {
  return (
    <section id={id} aria-labelledby={labelledBy} className="border-t border-border">
      <div className={`container-page py-20 lg:py-28 ${className}`}>{children}</div>
    </section>
  )
}

/** Supabase-style heading: one full line and one quieter line. */
export function Heading({
  id,
  strong,
  quiet,
  quietFirst = false,
  className = '',
}: {
  id: string
  strong: ReactNode
  quiet: ReactNode
  quietFirst?: boolean
  className?: string
}) {
  const a = <span className="block text-foreground">{strong}</span>
  const b = <span className="block text-foreground-lighter">{quiet}</span>
  return (
    <h2 id={id} className={`heading-section ${className}`}>
      {quietFirst ? (
        <>
          {b}
          {a}
        </>
      ) : (
        <>
          {a}
          {b}
        </>
      )}
    </h2>
  )
}

/** Muted copy with its key phrase at full strength (write it between **). */
export function Emphasis({ text }: { text: string }) {
  return (
    <>
      {text.split('**').map((part, i) =>
        i % 2 ? (
          <span key={i} className="text-foreground">
            {part}
          </span>
        ) : (
          part
        ),
      )}
    </>
  )
}
