import * as React from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import {
  ArrowDownToLine,
  Clock,
  Download,
  Eraser,
  RotateCw,
  Search,
  Server,
  WrapText,
  X,
} from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Toggle } from '@/components/ui/toggle'
import { Hint } from '@/components/ui/tooltip'
import type { LogEntry, LogStreamStatus } from '@/lib/api/useLogStream'
import { logTime } from '@/lib/format'
import { logSeverity } from '@/lib/logs'
import { cn } from '@/lib/utils'
import { useUi } from '@/stores/ui'

import type { StatusTone } from './status-tones'
import { StatePill } from './StatusBadge'

export interface LogViewerProps {
  lines: LogEntry[]
  status: LogStreamStatus
  error?: string | null
  /** Clears the lines received so far. */
  onClear?: () => void
  /** Reconnect / reload the stream. */
  onRestart?: () => void
  /** Instance column: `auto` shows it when any line has an instance. */
  showInstance?: boolean | 'auto'
  /** File name for Download (without extension). */
  downloadName?: string
  /** CSS height of the scroll area. */
  height?: string
  /** Shown when there are no lines yet. */
  emptyText?: React.ReactNode
  /** Extra toolbar controls (e.g. a tail-size select). */
  toolbarExtra?: React.ReactNode
  /** Accessible name of the log region. */
  label?: string
  className?: string
}

const STATUS_PILL: Record<LogStreamStatus, { tone: StatusTone; label: string; pulse: boolean }> = {
  idle: { tone: 'neutral', label: 'Idle', pulse: false },
  connecting: { tone: 'warning', label: 'Connecting', pulse: true },
  live: { tone: 'success', label: 'Live', pulse: true },
  ended: { tone: 'neutral', label: 'Ended', pulse: false },
  error: { tone: 'destructive', label: 'Error', pulse: false },
}

const ROW_HEIGHT = 20

const STATUS_ANNOUNCEMENT: Record<LogStreamStatus, string> = {
  idle: '',
  connecting: 'Connecting to the log stream',
  live: 'Log stream live',
  ended: 'Log stream ended',
  error: 'Log stream error',
}

/**
 * Supabase-style log viewer: mono 12.5px, faint surface, timestamp and
 * instance columns, lines coloured by severity (error / warning words, failed
 * `==>` system lines; plain stderr only gets a gutter mark), `==>` system lines
 * in green,
 * windowed rendering (thousands of lines), search filter with highlight,
 * follow (auto-scroll) that pauses when you scroll up, wrap / timestamps
 * toggles, clear and download.
 */
export function LogViewer({
  lines,
  status,
  error,
  onClear,
  onRestart,
  showInstance = 'auto',
  downloadName = 'logs',
  height = 'min(68dvh, 680px)',
  emptyText = 'No log lines yet.',
  toolbarExtra,
  label = 'Logs',
  className,
}: LogViewerProps) {
  const prefs = useUi((s) => s.logPrefs)
  const setPrefs = useUi((s) => s.setLogPrefs)
  const [query, setQuery] = React.useState('')
  const [follow, setFollow] = React.useState(true)
  const scrollRef = React.useRef<HTMLDivElement | null>(null)
  const searchId = React.useId()
  const [searchAnnouncement, setSearchAnnouncement] = React.useState('')

  const needle = query.trim().toLowerCase()
  const filtered = React.useMemo(
    () => (needle ? lines.filter((l) => l.text.toLowerCase().includes(needle) || (l.instance ?? '').includes(needle)) : lines),
    [lines, needle],
  )
  const hasInstance = React.useMemo(
    () => (showInstance === 'auto' ? lines.some((l) => l.instance) : showInstance),
    [lines, showInstance],
  )
  const instanceCol = hasInstance && prefs.instance

  // Screen readers: announce the match count once typing settles (not every streamed line).
  const matchCount = React.useRef(0)
  matchCount.current = filtered.length
  React.useEffect(() => {
    const t = window.setTimeout(
      () => setSearchAnnouncement(needle ? `${matchCount.current} matching line${matchCount.current === 1 ? '' : 's'}` : ''),
      needle ? 700 : 0,
    )
    return () => window.clearTimeout(t)
  }, [needle])

  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual returns non-memoizable functions by design
  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 24,
    getItemKey: (i) => filtered[i]?.seq ?? i,
  })

  // Re-measure when wrapping changes (row heights change).
  React.useEffect(() => {
    virtualizer.measure()
  }, [prefs.wrap, prefs.timestamps, instanceCol, virtualizer])

  // Auto-scroll to the newest line while following.
  React.useEffect(() => {
    if (!follow || filtered.length === 0) return
    virtualizer.scrollToIndex(filtered.length - 1, { align: 'end' })
    const el = scrollRef.current
    const raf = requestAnimationFrame(() => {
      if (el) el.scrollTop = el.scrollHeight
    })
    return () => cancelAnimationFrame(raf)
  }, [filtered.length, follow, virtualizer])

  const onScroll = () => {
    const el = scrollRef.current
    if (!el) return
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < ROW_HEIGHT * 1.5
    if (!atBottom && follow) setFollow(false)
    else if (atBottom && !follow) setFollow(true)
  }

  const download = () => {
    const text = lines
      .map((l) => `${l.ts}${l.instance ? ` [${l.instance}]` : ''}${l.stream === 'stderr' ? ' (stderr)' : ''} ${l.text}`)
      .join('\n')
    const blob = new Blob([text + '\n'], { type: 'text/plain;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `${downloadName}.log`
    document.body.appendChild(a)
    a.click()
    a.remove()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  }

  const pill = STATUS_PILL[status]
  const items = virtualizer.getVirtualItems()

  return (
    <div className={cn('flex min-w-0 flex-col overflow-hidden rounded-lg border bg-surface-100 shadow-card', className)}>
      {/* toolbar */}
      <div className="flex flex-wrap items-center gap-2 border-b px-2.5 py-2">
        <div className="relative w-full min-w-40 sm:w-60">
          <label htmlFor={searchId} className="sr-only">
            Filter log lines
          </label>
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-foreground-lighter" aria-hidden="true" />
          <Input
            id={searchId}
            size="sm"
            placeholder="Search logs…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="pr-7 pl-8"
          />
          {query && (
            <button
              type="button"
              aria-label="Clear search"
              onClick={() => setQuery('')}
              className="absolute top-1/2 right-1.5 flex size-5 -translate-y-1/2 items-center justify-center rounded text-foreground-lighter hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring outline-none"
            >
              <X className="size-3.5" />
            </button>
          )}
        </div>
        {toolbarExtra}
        <div className="flex items-center gap-0.5">
          <Hint label="Wrap lines">
            <Toggle size="default" aria-label="Wrap lines" pressed={prefs.wrap} onPressedChange={(wrap) => setPrefs({ wrap })}>
              <WrapText />
            </Toggle>
          </Hint>
          <Hint label="Show timestamps">
            <Toggle
              size="default"
              aria-label="Show timestamps"
              pressed={prefs.timestamps}
              onPressedChange={(timestamps) => setPrefs({ timestamps })}
            >
              <Clock />
            </Toggle>
          </Hint>
          {hasInstance && (
            <Hint label="Show instance">
              <Toggle
                size="default"
                className="hidden sm:inline-flex"
                aria-label="Show instance column"
                pressed={prefs.instance}
                onPressedChange={(instance) => setPrefs({ instance })}
              >
                <Server />
              </Toggle>
            </Hint>
          )}
        </div>
        <div className="ml-auto flex items-center gap-1.5">
          <StatePill tone={pill.tone} pulse={pill.pulse} label={pill.label} />
          <span className="hidden font-mono text-[11px] text-foreground-lighter tabular sm:inline">
            {needle ? `${filtered.length}/${lines.length}` : lines.length} lines
          </span>
          {/* Announce stream state changes and settled search results only, never the raw line count. */}
          <span role="status" className="sr-only">
            {STATUS_ANNOUNCEMENT[status]}
          </span>
          <span role="status" className="sr-only">
            {searchAnnouncement}
          </span>
          <Hint label={follow ? 'Following (scroll up to pause)' : 'Follow new lines'}>
            <Toggle
              size="default"
              aria-label="Follow new lines"
              pressed={follow}
              onPressedChange={(f) => {
                setFollow(f)
              }}
            >
              <ArrowDownToLine />
            </Toggle>
          </Hint>
          {onRestart && (
            <Hint label="Reconnect">
              <Button size="icon" variant="ghost" icon={<RotateCw />} aria-label="Reconnect log stream" onClick={onRestart} />
            </Hint>
          )}
          {onClear && (
            <Hint label="Clear">
              <Button size="icon" variant="ghost" icon={<Eraser />} aria-label="Clear logs" onClick={onClear} disabled={lines.length === 0} />
            </Hint>
          )}
          <Hint label="Download">
            <Button size="icon" variant="ghost" icon={<Download />} aria-label="Download logs" onClick={download} disabled={lines.length === 0} />
          </Hint>
        </div>
      </div>

      {error && status === 'error' && (
        <div role="alert" className="border-b border-destructive-border bg-destructive-soft px-3 py-2 text-[13px] text-destructive">
          {error}
        </div>
      )}

      {/* lines */}
      <div className="relative">
        <div
          ref={scrollRef}
          onScroll={onScroll}
          role="log"
          aria-label={label}
          aria-live="off"
          tabIndex={0}
          className={cn(
            'overflow-auto bg-log font-mono text-[12.5px] leading-5 outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset',
          )}
          style={{ height }}
        >
          {filtered.length === 0 ? (
            <div className="flex h-full items-center justify-center px-4 text-center font-sans text-[13px] text-foreground-lighter">
              {needle && lines.length > 0 ? 'No lines match your search.' : status === 'connecting' ? 'Connecting…' : emptyText}
            </div>
          ) : (
            <div className={cn('relative py-1.5', !prefs.wrap && 'w-max min-w-full')} style={{ height: virtualizer.getTotalSize() + 12 }}>
              {items.map((item) => {
                const l = filtered[item.index]
                if (!l) return null
                const severity = logSeverity(l.stream, l.text)
                return (
                  <div
                    key={item.key}
                    data-index={item.index}
                    data-stream={l.stream}
                    ref={virtualizer.measureElement}
                    className={cn(
                      'absolute left-0 flex w-full gap-3 px-3 hover:bg-surface-200',
                      // stderr is often plain progress (BuildKit, nginx): mark the stream in the gutter only
                      l.stream === 'stderr' && 'shadow-[inset_2px_0_0_var(--border-stronger)]',
                      severity === 'error' && 'bg-destructive/[0.06] shadow-[inset_2px_0_0_var(--destructive)]',
                      severity === 'warning' && 'shadow-[inset_2px_0_0_var(--warning)]',
                    )}
                    style={{ transform: `translateY(${item.start + 6}px)` }}
                  >
                    {prefs.timestamps && (
                      <time dateTime={l.ts} className="shrink-0 text-foreground-lighter select-none tabular" title={l.ts}>
                        <span className="sm:hidden">{logTime(l.ts, false)}</span>
                        <span className="hidden sm:inline">{logTime(l.ts)}</span>
                      </time>
                    )}
                    {instanceCol && (
                      <span className="hidden w-[6ch] shrink-0 text-foreground-lighter select-none sm:block">{l.instance ?? ''}</span>
                    )}
                    {l.stream === 'stderr' && <span className="sr-only">stderr: </span>}
                    <span
                      className={cn(
                        'min-w-0',
                        prefs.wrap ? 'break-all whitespace-pre-wrap' : 'whitespace-pre',
                        severity === 'error'
                          ? 'text-destructive'
                          : severity === 'warning'
                            ? 'text-warning'
                            : l.stream === 'system'
                              ? 'text-primary'
                              : 'text-foreground',
                      )}
                    >
                      {needle ? <Highlight text={l.text} needle={needle} /> : l.text || ' '}
                    </span>
                  </div>
                )
              })}
            </div>
          )}
        </div>
        {!follow && filtered.length > 0 && (
          <Button
            size="tiny"
            shape="pill"
            icon={<ArrowDownToLine />}
            className="absolute right-4 bottom-3 shadow-overlay"
            onClick={() => setFollow(true)}
          >
            Jump to latest
          </Button>
        )}
      </div>
    </div>
  )
}

function Highlight({ text, needle }: { text: string; needle: string }) {
  const lower = text.toLowerCase()
  const parts: React.ReactNode[] = []
  let i = 0
  let k = 0
  while (i < text.length) {
    const j = lower.indexOf(needle, i)
    if (j === -1) {
      parts.push(text.slice(i))
      break
    }
    if (j > i) parts.push(text.slice(i, j))
    parts.push(
      <mark key={k++} className="rounded-[2px] bg-warning/30 text-inherit">
        {text.slice(j, j + needle.length)}
      </mark>,
    )
    i = j + needle.length
  }
  return <>{parts}</>
}
