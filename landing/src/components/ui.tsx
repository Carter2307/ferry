import type { ReactNode } from 'react'

import { Divider } from './backdrop'
import { Reveal } from './Reveal'

/** A page section: a full-width hairline on top (with its beam), the content in the page container. */
export function Section({
  id,
  labelledBy,
  beam = '0s',
  reverse = false,
  className = '',
  children,
}: {
  id?: string
  labelledBy: string
  /** When the beam of the hairline starts (see `Divider`). */
  beam?: string
  reverse?: boolean
  className?: string
  children: ReactNode
}) {
  return (
    <section id={id} aria-labelledby={labelledBy}>
      <Divider delay={beam} reverse={reverse} />
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
  const a = (delay: number) => (
    <Reveal as="span" delay={delay} className="block text-foreground">
      {strong}
    </Reveal>
  )
  const b = (delay: number) => (
    <Reveal as="span" delay={delay} className="block text-foreground-lighter">
      {quiet}
    </Reveal>
  )
  return (
    <h2 id={id} className={`heading-section ${className}`}>
      {quietFirst ? (
        <>
          {b(0)}
          {a(0.07)}
        </>
      ) : (
        <>
          {a(0)}
          {b(0.07)}
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
