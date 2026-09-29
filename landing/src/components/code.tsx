import { Check, Copy } from 'lucide-react'
import { useEffect, useState } from 'react'

export function CopyButton({ text, label, className = '' }: { text: string; label: string; className?: string }) {
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    if (!copied) return
    const id = window.setTimeout(() => setCopied(false), 1600)
    return () => window.clearTimeout(id)
  }, [copied])

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
    } catch {
      // No clipboard access (insecure context, denied permission): the text stays selectable.
    }
  }

  return (
    <button
      type="button"
      onClick={copy}
      aria-label={label}
      className={`grid size-8 shrink-0 place-items-center rounded-md border border-transparent text-foreground-lighter transition-colors hover:border-border-strong hover:bg-surface-300 hover:text-foreground ${className}`}
    >
      {copied ? (
        <Check className="size-3.5 text-success" aria-hidden="true" />
      ) : (
        <Copy className="size-3.5" aria-hidden="true" />
      )}
      <span className="sr-only" aria-live="polite">
        {copied ? 'Copied' : ''}
      </span>
    </button>
  )
}

/** Commands run one after the other; the copy button copies them without the prompts. */
export function CommandBlock({ lines, label }: { lines: string[]; label: string }) {
  return (
    <div className="relative rounded-lg border border-border bg-log">
      <pre className="no-scrollbar overflow-x-auto py-3.5 pr-14 pl-4 text-[12.5px] leading-[1.8] text-foreground-light">
        {lines.map((line) => (
          <span key={line} className="block">
            <span className="select-none text-foreground-muted" aria-hidden="true">
              ${' '}
            </span>
            {line}
          </span>
        ))}
      </pre>
      <CopyButton text={lines.join('\n')} label={label} className="absolute top-2 right-2" />
    </div>
  )
}

/** A short CLI transcript: the command, then its output (`==>` lines from the server). */
export function Transcript({ command, output }: { command: string; output: string[] }) {
  return (
    <pre className="py-4 pr-14 pl-5 text-[12.5px] leading-[1.75] whitespace-pre-wrap text-foreground-light [overflow-wrap:anywhere]">
      <span className="block text-foreground">
        <span className="select-none text-foreground-muted" aria-hidden="true">
          ${' '}
        </span>
        {command}
      </span>
      {output.map((line, i) =>
        line.startsWith('==>') ? (
          <span key={i} className={`block ${/live|passed|successful|Available/.test(line) ? 'text-success' : ''}`}>
            <span className="text-primary">==&gt;</span>
            {line.slice(3)}
          </span>
        ) : (
          <span key={i} className="block text-foreground-lighter">
            {line}
          </span>
        ),
      )}
    </pre>
  )
}
