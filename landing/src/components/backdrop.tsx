import { PixelField } from 'ferry-shaders'
import type { CSSProperties } from 'react'

import { useTheme } from '@/theme'

/** Inline custom properties of one beam: how long one pass takes, when it starts, how far it goes. */
type BeamStyle = CSSProperties & { '--duration'?: string; '--delay'?: string; '--travel'?: string }

/**
 * A dense field of small pixels at an edge of the page, which thins out into the page and moves
 * slowly: the pixel field of ferry-shaders (a WebGPU shader), in Ferry's tokens. The shader reads
 * the tokens again when the theme changes, stops when it is out of view, and draws one still frame
 * for a visitor who asks for reduced motion. A browser with no WebGPU gets a still pixel pattern
 * in CSS (`.pixel-fallback`).
 */
function Field({ from, falloff = 2.6, className }: { from: 'top' | 'bottom'; falloff?: number; className: string }) {
  const dark = useTheme().theme === 'dark'
  return (
    <PixelField
      data-from={from}
      origin={from}
      falloff={falloff}
      color="var(--foreground)"
      accent="var(--primary-bright)"
      // Light pixels on a dark canvas read stronger than dark pixels on white: lower them.
      opacity={dark ? 0.1 : 0.13}
      accentOpacity={dark ? 0.42 : 0.5}
      className={`pixel-field ${className}`}
      fallback={<div className="pixel-fallback absolute inset-0" />}
    />
  )
}

/** The background of the top of the page: a pixel field that starts at the very top and thins out downward. */
export function HeroBackdrop() {
  return <Field from="top" className="absolute inset-x-0 top-0 -z-10 h-[46rem]" />
}

/**
 * The background of the end of the page: a pixel field that starts at the very bottom of the page
 * and thins out upward. It is dense in the free band under the footer, it mixes with the footer,
 * and its last scattered pixels reach the closing title above the footer. The footer has no
 * background of its own, so the field shows through it.
 */
export function PageEndBackdrop() {
  return <Field from="bottom" falloff={3} className="absolute inset-x-0 bottom-0 -z-10 h-[64rem]" />
}

/**
 * Two hairlines down the whole page, at the edges of the page column, each with slow beams: short
 * lines of light that run along the hairline. The motion is a CSS animation on `transform` (see
 * `.rail-runner` in index.css), and it stops for visitors who ask for reduced motion. The rails
 * show on screens wide enough to have a margin outside the column.
 */
export function PageRails() {
  return (
    <div
      aria-hidden="true"
      className="pointer-events-none absolute inset-0 -z-10 hidden overflow-hidden min-[86rem]:block"
    >
      <div className="relative mx-auto h-full max-w-[80rem]">
        {(['left-0', 'right-0'] as const).map((side, index) => (
          <div key={side} className={`absolute inset-y-0 w-px bg-border ${side}`}>
            {[0, 1, 2].map((beam) => (
              <span
                key={beam}
                className="rail-runner"
                style={{ '--duration': '84s', '--delay': `${-(beam * 28 + index * 14)}s` } as BeamStyle}
              >
                <span className="beam-segment" />
              </span>
            ))}
          </div>
        ))}
      </div>
    </div>
  )
}

/** The hairline between two sections, with one beam that runs along it. */
export function Divider({
  delay = '0s',
  reverse = false,
  className = '',
}: {
  /** When the beam starts. A negative value starts it mid-run, so two lines are not in step. */
  delay?: string
  /** The beam runs from right to left. */
  reverse?: boolean
  className?: string
}) {
  const style: BeamStyle = { top: 0, '--duration': '16s', '--delay': delay, '--travel': '100vw' }
  return (
    <div aria-hidden="true" className={`relative h-px overflow-hidden bg-border ${className}`}>
      <span className={`beam beam-x ${reverse ? 'beam-reverse' : ''}`} style={style} />
    </div>
  )
}
