import { useCallback, useEffect, useRef, useState, type RefObject } from 'react'

export type ClockStatus = 'waiting' | 'playing' | 'paused' | 'done'

const reducedMotion = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches

function initialState(end: number): { t: number; status: ClockStatus } {
  if (import.meta.env.DEV) {
    // Dev aid: `?t=5000` freezes the demo 5 s in.
    const t = Number(new URLSearchParams(window.location.search).get('t'))
    if (t > 0) return { t: Math.min(t, end), status: 'paused' }
  }
  return reducedMotion() ? { t: end, status: 'done' } : { t: 0, status: 'waiting' }
}

/**
 * The demo's clock, in ms. It starts the first time `target` (the demo's window) is on screen,
 * runs once up to `end`, and can be paused or replayed. With reduced motion it
 * stays on the final frame. `rate` gives how fast it runs on each frame: the harbor figure
 * slows it while the pointer is on it.
 */
export function useDeployClock(end: number, target: RefObject<Element | null>, rate: RefObject<() => number>) {
  const [reduced] = useState(reducedMotion)
  const [start] = useState(() => initialState(end))
  const [t, setT] = useState(start.t)
  const [status, setStatus] = useState<ClockStatus>(start.status)
  const time = useRef(t)

  useEffect(() => {
    const el = target.current
    if (status !== 'waiting' || !el) return
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) setStatus('playing')
      },
      { threshold: 0.2 },
    )
    io.observe(el)
    return () => io.disconnect()
  }, [status, target])

  useEffect(() => {
    if (status !== 'playing') return
    let frame = 0
    let last = performance.now()
    const tick = (now: number) => {
      // Clamp the step so a background tab doesn't skip half the deploy.
      time.current = Math.min(time.current + Math.min(now - last, 64) * rate.current(), end)
      last = now
      setT(time.current)
      if (time.current >= end) setStatus('done')
      else frame = requestAnimationFrame(tick)
    }
    frame = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(frame)
  }, [status, end, rate])

  const pause = useCallback(() => setStatus((s) => (s === 'playing' ? 'paused' : s)), [])
  const play = useCallback(() => setStatus((s) => (s === 'paused' || s === 'waiting' ? 'playing' : s)), [])
  const replay = useCallback(() => {
    time.current = 0
    setT(0)
    setStatus('playing')
  }, [])

  return { t, status, reduced, pause, play, replay }
}
