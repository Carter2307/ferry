import { Fragment, useLayoutEffect, useRef, useState, type ReactNode } from 'react'

const YAML = `services:
  - type: web
    name: shop
    runtime: node
    repo: https://github.com/acme/shop
    buildCommand: npm ci && npm run build
    startCommand: npm start
    healthCheckPath: /healthz
    numInstances: 2
    domains: [shop.example.com]
    envVars:
      - key: DATABASE_URL
        fromDatabase: { name: shop-db, property: connectionString }

  - type: cron
    name: nightly-report
    runtime: node
    repo: https://github.com/acme/shop
    schedule: "0 3 * * *"
    startCommand: node report.js

databases:
  - name: shop-db`.split('\n')

/** Notes on line ranges (1-based, inclusive). `code` spans are written in backticks. */
const NOTES = [
  { from: 2, to: 7, text: 'Built from your repository. With the GitHub webhook set up, every push deploys it.' },
  {
    from: 8,
    to: 10,
    text: 'Two instances on your domain, with HTTPS. A deploy gets traffic only once `/healthz` answers.',
  },
  { from: 11, to: 13, text: 'Connection strings are references, filled in when the containers start.' },
  { from: 15, to: 20, text: 'Runs every night at 03:00 UTC. `ferry run nightly-report` runs it right now.' },
  { from: 22, to: 23, text: 'Postgres, with its data on a persistent volume.' },
]

const LINE_H = 20
const PAD_Y = 16
const NOTE_GAP = 10
const GUTTER = 40
const lineTop = (line: number) => PAD_Y + (line - 1) * LINE_H

function withCode(text: string): ReactNode {
  return text.split('`').map((part, i) =>
    i % 2 ? (
      <code key={i} className="inline-code">
        {part}
      </code>
    ) : (
      <Fragment key={i}>{part}</Fragment>
    ),
  )
}

const punct = (s: string) => <span className="text-foreground-muted">{s}</span>

/** Just enough YAML highlighting for this file: keys, punctuation, service types. */
function yamlLine(line: string): ReactNode {
  const m = /^(\s*)(- )?(?:([A-Za-z]+)(:))?(.*)$/.exec(line)
  if (!m) return line
  const [, indent = '', dash, key, colon, rest = ''] = m
  let value: ReactNode = rest
  if (rest.trim().startsWith('{') || rest.trim().startsWith('[')) {
    value = rest.split(/([{}[\],]|\b[A-Za-z]+:)/).map((tok, i) =>
      /^[{}[\],]$/.test(tok) ? (
        <Fragment key={i}>{punct(tok)}</Fragment>
      ) : tok.endsWith(':') ? (
        <span key={i} className="text-primary">
          {tok}
        </span>
      ) : (
        <Fragment key={i}>{tok}</Fragment>
      ),
    )
  } else if (key === 'type') {
    value = <span className="font-semibold text-foreground">{rest}</span>
  }
  return (
    <>
      {indent}
      {dash && punct(dash)}
      {key && <span className="text-primary">{key}</span>}
      {colon && punct(colon)}
      {value}
    </>
  )
}

export function BlueprintFile() {
  const notes = useRef<(HTMLDivElement | null)[]>([])
  const [tops, setTops] = useState(() => NOTES.map((n) => lineTop(n.from)))
  const [active, setActive] = useState<number | null>(null)

  // Notes sit next to their first line, pushed down when the one above is long.
  useLayoutEffect(() => {
    const layout = () => {
      let bottom = -Infinity
      const next = NOTES.map((n, i) => {
        const top = Math.max(lineTop(n.from), bottom + NOTE_GAP)
        bottom = top + (notes.current[i]?.offsetHeight ?? 0)
        return top
      })
      setTops((prev) => (prev.every((v, i) => v === next[i]) ? prev : next))
    }
    layout()
    const ro = new ResizeObserver(layout)
    notes.current.forEach((el) => el && ro.observe(el))
    return () => ro.disconnect()
  }, [])

  const noteAt = new Map(NOTES.map((n, i) => [n.from, i]))
  const activeNote = active === null ? undefined : NOTES[active]

  return (
    <div className="grid h-full min-h-0 lg:grid-cols-[17rem_minmax(0,1fr)]">
      {/* Margin notes (wide screens). */}
      <div className="relative border-r border-border bg-surface-75 max-lg:hidden" aria-hidden="true">
        {NOTES.map((n, i) => {
          const drop = tops[i]! - lineTop(n.from)
          return (
            <div
              key={n.from}
              ref={(el) => {
                notes.current[i] = el
              }}
              onMouseEnter={() => setActive(i)}
              onMouseLeave={() => setActive(null)}
              className={`absolute left-6 text-[13px] leading-[20px] transition-colors ${
                active === i ? 'text-foreground' : 'text-foreground-lighter'
              }`}
              style={{ top: tops[i], right: GUTTER }}
            >
              {withCode(n.text)}
              <svg
                className="absolute top-0 h-px overflow-visible"
                style={{ right: -GUTTER, width: GUTTER - 8, transform: `translateY(${LINE_H / 2}px)` }}
              >
                <path
                  d={`M 0 0 H 12 L 20 ${-drop} H ${GUTTER - 8}`}
                  fill="none"
                  strokeWidth={1.5}
                  className={active === i ? 'stroke-primary' : 'stroke-border-stronger'}
                />
              </svg>
            </div>
          )
        })}
      </div>

      <figure className="relative min-h-0 min-w-0 bg-log">
        <figcaption className="sr-only">render.yaml for a web service, a cron job and a Postgres database</figcaption>
        <div className="no-scrollbar relative h-full overflow-auto font-mono text-[12.5px] text-foreground-light">
          <pre className="leading-[20px]" style={{ paddingBlock: PAD_Y }}>
            {YAML.map((line, i) => {
              const n = i + 1
              const note = noteAt.get(n)
              const lit = activeNote && n >= activeNote.from && n <= activeNote.to
              return (
                <Fragment key={n}>
                  {note !== undefined && (
                    // A YAML comment on small screens; wrapped lines hang after the `# `.
                    <span
                      className="block whitespace-pre-wrap pr-5 text-foreground-muted lg:hidden"
                      style={{
                        paddingLeft: `calc(1.5rem + ${(line.match(/^\s*/)?.[0].length ?? 0) + 2}ch)`,
                        textIndent: '-2ch',
                      }}
                    >
                      # {NOTES[note]!.text.replaceAll('`', '')}
                    </span>
                  )}
                  <span
                    className={`block pl-6 pr-5 whitespace-pre ${lit ? 'bg-primary-soft' : ''}`}
                    style={{ height: LINE_H }}
                  >
                    {yamlLine(line)}
                  </span>
                </Fragment>
              )
            })}
          </pre>
          {NOTES.map((n, i) => (
            <span
              key={n.from}
              aria-hidden="true"
              className={`absolute left-2.5 w-[3px] rounded-full transition-colors max-lg:hidden ${
                active === i ? 'bg-primary' : 'bg-border-stronger'
              }`}
              style={{ top: lineTop(n.from) + 3, height: (n.to - n.from + 1) * LINE_H - 6 }}
            />
          ))}
        </div>
      </figure>
    </div>
  )
}
