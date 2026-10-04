import { Lock } from 'lucide-react'
import { useEffect, useRef, type RefObject } from 'react'

import { mountSlip, type SlipHandle } from './hairline'
import { SCENE } from './timeline'

/** How fast the clock runs while the pointer is on the figure. */
const SLOW = 0.25
const FULL_SPEED = () => 1

const DESCRIPTION =
  'A ferry slip. The new deploy docks next to the live one. Once its health check passes, the requests go to it in one step. The old deploy answers the requests it already had, then leaves.'

/** What the deploy does at `t`, said the way the log next to the figure says it. */
function caption(t: number) {
  if (t < SCENE.boatBEnter) return 'One deploy is live'
  if (t < SCENE.boatBDock) return 'Building the new deploy'
  if (t < SCENE.healthStart) return 'Starting the new deploy'
  if (t < SCENE.healthy) return 'Checking its health'
  if (t < SCENE.switchAt) return 'Health check passed'
  if (t < SCENE.stopA) return 'Requests go to the new deploy'
  if (t < SCENE.boatAGone) return 'The old deploy leaves'
  return 'The new deploy is live'
}

/**
 * The deploy as a ferry slip, drawn by the Hairline figure in ./hairline. The live ferry has the
 * bright stroke and takes the cars (the requests). The figure reads the demo's clock, so it shows
 * the same moment as the terminal. While the pointer is on it, it asks for a slower clock through
 * `rate`, so the handover can be read.
 */
export function HarborFigure({
  t,
  rate,
  className = '',
}: {
  /** The demo's clock, in ms. */
  t: number
  /** The figure puts here how fast the clock must run: `useDeployClock` reads it on every frame. */
  rate: RefObject<() => number>
  className?: string
}) {
  const stage = useRef<HTMLDivElement>(null)
  const handle = useRef<SlipHandle | null>(null)
  const time = useRef(t)

  useEffect(() => {
    time.current = t
    handle.current?.seek(t)
  }, [t])

  useEffect(() => {
    const element = stage.current
    if (!element) return
    let gone = false
    mountSlip(element, SLOW).then(
      (mounted) => {
        if (gone) {
          mounted.destroy()
          return
        }
        handle.current = mounted
        mounted.seek(time.current)
        rate.current = mounted.rate
      },
      () => {
        // The figure did not load: the panel stays empty, and the terminal still tells the deploy.
      },
    )
    return () => {
      gone = true
      handle.current?.destroy()
      handle.current = null
      rate.current = FULL_SPEED
    }
  }, [rate])

  return (
    <div className={`text-[13px] ${className}`}>
      <div ref={stage} data-hairline="slip" role="img" aria-label={DESCRIPTION} className="harbor-figure" />
      <p aria-hidden="true" className="pointer-events-none absolute top-3.5 left-5 flex items-center gap-1.5 text-foreground-light">
        <Lock className="size-3.5 text-foreground-lighter" />
        my-app.example.com
      </p>
      <p aria-hidden="true" className="pointer-events-none absolute bottom-3.5 left-5 text-foreground-lighter">
        {caption(t)}
      </p>
    </div>
  )
}
