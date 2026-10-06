import * as m from 'motion/react-m'
import type { ReactNode } from 'react'

/** A soft ease-out: fast at the start, a long calm end. */
const EASE = [0.22, 1, 0.36, 1] as const

/**
 * Makes its content appear with a short fade and a small rise. Visitors who ask for reduced motion
 * get the fade only (see the `MotionConfig` in App.tsx). It is the light `m` component of Motion:
 * the `LazyMotion` in App.tsx brings the animation code.
 */
export function Reveal({
  children,
  delay = 0,
  y = 14,
  immediate = false,
  as = 'div',
  className,
}: {
  children: ReactNode
  /** Seconds to wait before the element appears. Use small steps (0.06 to 0.1) to stagger siblings. */
  delay?: number
  /** Distance the element rises, in pixels. */
  y?: number
  /** Appear as soon as the page loads (the top of the page) instead of when it scrolls into view. */
  immediate?: boolean
  /** `span` for a line inside a heading. */
  as?: 'div' | 'span'
  className?: string
}) {
  const shown = { opacity: 1, y: 0 }
  const Element = as === 'span' ? m.span : m.div
  return (
    <Element
      className={className}
      initial={{ opacity: 0, y }}
      {...(immediate
        ? { animate: shown }
        : { whileInView: shown, viewport: { once: true, margin: '0px 0px -12% 0px' } })}
      transition={{ duration: 0.7, delay, ease: EASE }}
    >
      {children}
    </Element>
  )
}
