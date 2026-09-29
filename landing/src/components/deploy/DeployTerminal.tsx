import { memo, useEffect, useRef } from 'react'

import { COMMAND, LINES, TRANSCRIPT, type Tone } from './timeline'

const TONE: Record<Tone, string> = {
  plain: 'text-foreground-light',
  system: 'text-foreground-light',
  dim: 'text-foreground-muted',
  ok: 'text-success',
  wait: 'text-warning',
}

/**
 * The typed command stays pinned on top; the output below follows the newest
 * line, like `--follow`. Screen readers get the whole transcript at once.
 */
export const DeployTerminal = memo(function DeployTerminal({
  typed,
  shown,
  cursor,
  className = '',
}: {
  typed: number
  shown: number
  cursor: boolean
  className?: string
}) {
  const output = useRef<HTMLDivElement>(null)
  const lines = useRef<HTMLDivElement>(null)

  // Follow the newest line, also when a late web font changes the wrapping.
  useEffect(() => {
    const box = output.current
    const content = lines.current
    if (!box || !content) return
    const ro = new ResizeObserver(() => {
      box.scrollTop = box.scrollHeight
    })
    ro.observe(content)
    return () => ro.disconnect()
  }, [])

  return (
    <figure className={`flex min-h-0 flex-col bg-log font-mono text-[12px] leading-[1.7] ${className}`}>
      <figcaption className="sr-only">Output of ferry up, deploying the app in the current folder</figcaption>
      <pre className="sr-only">{TRANSCRIPT}</pre>
      <div aria-hidden="true" className="border-b border-border px-5 py-3.5 text-[13px] text-foreground">
        <span className="select-none text-foreground-muted">$ </span>
        {COMMAND.slice(0, typed)}
        {cursor && <span className="terminal-cursor" />}
      </div>
      <div ref={output} aria-hidden="true" className="terminal-output min-h-0 flex-1 overflow-hidden px-5 py-4">
        <div ref={lines}>
          {LINES.slice(0, shown).map((line) => (
            <div key={line.at} className={`whitespace-pre-wrap break-words ${TONE[line.tone]}`}>
              {line.tone === 'system' ? (
                <>
                  <span className="text-primary">==&gt;</span>
                  {line.text.slice(3)}
                </>
              ) : (
                line.text
              )}
            </div>
          ))}
        </div>
      </div>
    </figure>
  )
})
