import { useEffect, useRef, useState, useSyncExternalStore, type KeyboardEvent } from 'react'

import { BlueprintFile } from './BlueprintFile'
import { DashboardMock } from './DashboardMock'
import { DeployTerminal } from './deploy/DeployTerminal'
import { HarborScene } from './deploy/HarborScene'
import { DONE, TYPE_END, typedChars, visibleLines } from './deploy/timeline'
import { useDeployClock, type ClockStatus } from './deploy/useDeployClock'
import { Reveal } from './Reveal'

const TABS = [
  { id: 'cli', label: 'Command line' },
  { id: 'dashboard', label: 'Dashboard' },
  { id: 'blueprint', label: 'Blueprint' },
] as const

type TabId = (typeof TABS)[number]['id']

const NOTES: Record<TabId, string> = {
  cli: 'ferry up packs the folder, builds it and starts the new deploy next to the live one. Once its health check passes, the proxy sends it every new request in one step, and the old deploy stops after answering the requests it already had.',
  dashboard:
    'The dashboard ships inside ferryd: services, deploys, logs, environment variables and datastores, updated live.',
  blueprint:
    'Apply the file with ferry blueprint apply render.yaml. Ferry creates the databases, then the services, and deploys them. Keys it doesn’t support are skipped with a warning.',
}

function PlaybackButton({
  status,
  onPause,
  onPlay,
  onReplay,
}: {
  status: ClockStatus
  onPause: () => void
  onPlay: () => void
  onReplay: () => void
}) {
  const { label, onClick, icon } =
    status === 'playing'
      ? { label: 'Pause', onClick: onPause, icon: <path d="M5 3.5v9M11 3.5v9" strokeWidth="2" strokeLinecap="round" /> }
      : status === 'done'
        ? {
            label: 'Replay',
            onClick: onReplay,
            icon: (
              <path
                d="M3.5 8a4.5 4.5 0 1 0 1.4-3.3M3.5 2.75v2.5H6"
                strokeWidth="1.6"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            ),
          }
        : {
            label: 'Play',
            onClick: onPlay,
            icon: <path d="M5 3.25v9.5L12.5 8 5 3.25Z" strokeWidth="1.6" strokeLinejoin="round" />,
          }
  return (
    <button type="button" onClick={onClick} className="btn btn-secondary btn-sm shrink-0">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" aria-hidden="true">
        {icon}
      </svg>
      {label}
    </button>
  )
}

const PHONE = '(max-width: 639px)'
const subscribePhone = (onChange: () => void) => {
  const mq = window.matchMedia(PHONE)
  mq.addEventListener('change', onChange)
  return () => mq.removeEventListener('change', onChange)
}
const isPhone = () => window.matchMedia(PHONE).matches

/**
 * The product at work, right under the headline: the `ferry up` demo, the dashboard and a
 * blueprint, in one window.
 */
export function Showcase() {
  const phone = useSyncExternalStore(subscribePhone, isPhone)
  const [tab, setTab] = useState<TabId>('cli')
  const frame = useRef<HTMLDivElement>(null)
  const clock = useDeployClock(DONE, frame)
  const { t, status, pause, play } = clock

  // The demo only runs while its tab is showing: leaving the tab pauses a run,
  // coming back resumes it (a run the visitor paused stays paused).
  const pausedByTab = useRef(false)
  useEffect(() => {
    if (tab !== 'cli' && status === 'playing') {
      pausedByTab.current = true
      pause()
    } else if (tab === 'cli' && pausedByTab.current) {
      pausedByTab.current = false
      play()
    }
  }, [tab, status, pause, play])

  const tabRefs = useRef<(HTMLButtonElement | null)[]>([])
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0
    if (!step) return
    e.preventDefault()
    const next = (i + step + TABS.length) % TABS.length
    setTab(TABS[next]!.id)
    tabRefs.current[next]?.focus()
  }

  return (
    <section id="tour" aria-labelledby="tour-title" className="container-page pb-20 lg:pb-28">
      <h2 id="tour-title" className="sr-only">
        Deploy from your terminal and watch it go live
      </h2>
      <Reveal immediate delay={0.32} y={20}>
        <div role="tablist" aria-label="Ways to use Ferry" className="flex flex-wrap gap-2">
          {TABS.map((item, i) => (
            <button
              key={item.id}
              ref={(el) => {
                tabRefs.current[i] = el
              }}
              type="button"
              role="tab"
              id={`tab-${item.id}`}
              aria-selected={tab === item.id}
              aria-controls={`panel-${item.id}`}
              tabIndex={tab === item.id ? 0 : -1}
              onClick={() => setTab(item.id)}
              onKeyDown={(e) => onKeyDown(e, i)}
              className={`btn btn-sm ${
                tab === item.id
                  ? 'border-border-stronger bg-surface-300 text-foreground'
                  : 'border-border text-foreground-lighter hover:border-border-strong hover:text-foreground'
              }`}
            >
              {item.label}
            </button>
          ))}
        </div>

        <div
          ref={frame}
          className="mt-4 overflow-hidden rounded-xl border border-border-strong bg-surface-100 shadow-[var(--shadow-overlay)]"
        >
          <div className="flex h-9 items-center gap-1.5 border-b border-border px-4" aria-hidden="true">
            {[0, 1, 2].map((i) => (
              <span key={i} className="size-2.5 rounded-full bg-border-stronger" />
            ))}
          </div>
          <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} className="h-[36rem] sm:h-[32rem]">
            {tab === 'cli' && (
              <div className="grid h-full grid-rows-[13rem_minmax(0,1fr)] lg:grid-cols-[minmax(0,5fr)_minmax(0,7fr)] lg:grid-rows-1">
                <DeployTerminal
                  typed={typedChars(t)}
                  shown={visibleLines(t)}
                  cursor={t < TYPE_END + 300}
                  className="border-border max-lg:order-2 max-lg:border-t lg:border-r"
                />
                <div className="relative min-h-0 overflow-hidden bg-surface-75">
                  <HarborScene t={t} compact={phone} className="absolute inset-0 h-full w-full" />
                </div>
              </div>
            )}
            {tab === 'dashboard' && <DashboardMock />}
            {tab === 'blueprint' && <BlueprintFile />}
          </div>
        </div>

        <div className="mt-5 flex flex-wrap items-start justify-between gap-4">
          <p className="max-w-[46rem] text-sm text-foreground-lighter">{NOTES[tab]}</p>
          {tab === 'cli' && !clock.reduced && (
            <PlaybackButton status={clock.status} onPause={clock.pause} onPlay={clock.play} onReplay={clock.replay} />
          )}
        </div>
      </Reveal>
    </section>
  )
}
