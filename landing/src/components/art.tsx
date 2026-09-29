import { Check, Loader, Lock } from 'lucide-react'
import { siPostgresql, siRedis } from 'simple-icons'

import { BrandIcon } from './BrandIcon'

/*
 * Card illustrations: UI fragments and line art in the card's greys, with the
 * blue and green only where they mean something. They fade into the card.
 */

const MARK = [
  'M4 13.5h16l-2.2 4.2a2 2 0 0 1-1.77 1.07H7.97A2 2 0 0 1 6.2 17.7L4 13.5Z',
  'M7.5 13.5V9.25c0-.69.56-1.25 1.25-1.25h6.5c.69 0 1.25.56 1.25 1.25v4.25',
  'M12 8V4.5',
  'M3 21.25c1.5 0 1.5-.9 3-.9s1.5.9 3 .9 1.5-.9 3-.9 1.5.9 3 .9 1.5-.9 3-.9 1.5.9 3 .9',
]

/** The Ferry mark as a double outline on a construction grid. */
export function ArtFerry() {
  return (
    <svg viewBox="0 0 260 260" fill="none" aria-hidden="true" className="h-full w-full">
      <g stroke="var(--border-strong)" strokeDasharray="3 5">
        <line x1="130" y1="0" x2="130" y2="260" />
        <line x1="0" y1="130" x2="260" y2="130" />
        <circle cx="130" cy="130" r="104" />
      </g>
      <rect x="30" y="30" width="200" height="200" rx="46" fill="var(--surface-75)" stroke="var(--border-stronger)" />
      <rect x="42" y="42" width="176" height="176" rx="38" stroke="var(--border)" />
      <g transform="translate(40 33) scale(7.5)" strokeLinecap="round" strokeLinejoin="round">
        {MARK.map((d) => (
          <path
            key={d}
            d={d}
            vectorEffect="non-scaling-stroke"
            strokeWidth={6}
            className="stroke-foreground-muted transition-colors duration-300 group-hover:stroke-primary"
          />
        ))}
        {MARK.map((d) => (
          <path key={`in-${d}`} d={d} vectorEffect="non-scaling-stroke" strokeWidth={3.5} stroke="var(--surface-75)" />
        ))}
      </g>
    </svg>
  )
}

const SERVICES = [
  { name: 'shop', type: 'Web service', live: true },
  { name: 'api', type: 'Private service', live: true },
  { name: 'mailer', type: 'Background worker', live: true },
  { name: 'nightly-report', type: 'Cron job', live: false },
  { name: 'docs', type: 'Static site', live: true },
]

export function ArtServices() {
  return (
    <div className="fade-edges absolute inset-x-0 bottom-0 top-[40%] flex flex-col justify-center gap-2 px-5">
      {SERVICES.map((s, i) => (
        <div
          key={s.name}
          className={`flex items-center gap-2.5 rounded-lg border border-border bg-surface-75 px-3 py-2 text-[13px] ${
            i % 2 ? 'ml-6 -mr-6' : '-ml-2 mr-2'
          }`}
        >
          <span className={`size-1.5 shrink-0 rounded-full ${s.live ? 'bg-success' : 'bg-foreground-muted'}`} />
          <span className="font-medium text-foreground-light">{s.name}</span>
          <span className="ml-auto truncate text-foreground-muted">{s.type}</span>
        </div>
      ))}
    </div>
  )
}

export function ArtDatastores() {
  return (
    <div className="absolute inset-x-0 bottom-0 top-[42%] flex flex-col justify-center gap-4 px-5">
      <div className="fade-right overflow-hidden whitespace-nowrap rounded-lg border border-border bg-surface-75 px-3 py-2 font-mono text-[12px] text-foreground-lighter">
        DATABASE_URL=<span className="text-primary">{'${{datastore.shop-db.connectionString}}'}</span>
      </div>
      <div className="grid grid-cols-2 gap-2">
        {[
          { icon: siPostgresql, name: 'shop-db', kind: 'Postgres 16' },
          { icon: siRedis, name: 'cache', kind: 'Redis' },
        ].map((d) => (
          <div key={d.name} className="rounded-lg border border-border bg-surface-75 p-3">
            <BrandIcon
              icon={d.icon}
              className="size-5 text-foreground-muted transition-colors group-hover:text-foreground-light"
            />
            <div className="mt-3 text-[13px] font-medium text-foreground-light">{d.name}</div>
            <div className="mt-0.5 flex items-center gap-1.5 text-[12px] text-foreground-muted">
              <span className="size-1.5 rounded-full bg-success" />
              {d.kind}
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}

const DOMAINS = [
  { host: 'shop.example.com', ready: true },
  { host: 'api.example.com', ready: true },
  { host: 'status.example.com', ready: false },
]

export function ArtDomains() {
  return (
    <div className="fade-edges absolute inset-x-0 bottom-0 top-[42%] flex flex-col justify-center gap-2 px-5">
      {DOMAINS.map((d, i) => (
        <div
          key={d.host}
          className={`flex items-center gap-2 rounded-lg border border-border bg-surface-75 px-3 py-2 text-[13px] ${
            i === 1 ? 'ml-5 -mr-3' : ''
          }`}
        >
          <Lock
            className={`size-3.5 shrink-0 ${d.ready ? 'text-success' : 'text-foreground-muted'}`}
            aria-hidden="true"
          />
          <span className="truncate text-foreground-light">{d.host}</span>
          <span className="ml-auto flex shrink-0 items-center gap-1 text-[12px] text-foreground-muted">
            {d.ready ? (
              <Check className="size-3.5 text-success" aria-hidden="true" />
            ) : (
              <>
                <Loader className="size-3.5" aria-hidden="true" />
                Issuing
              </>
            )}
          </span>
        </div>
      ))}
    </div>
  )
}

/** Fourteen nights of a cron job: one failed run among the successes. */
const RUNS = [1, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1]

export function ArtCron() {
  return (
    <div className="absolute inset-x-0 bottom-0 top-[42%] flex flex-col justify-center gap-5 px-5">
      <div className="self-start rounded-lg border border-border bg-surface-75 px-3 py-2 font-mono text-[12px] text-foreground-lighter">
        schedule: <span className="text-foreground">"0 3 * * *"</span>
      </div>
      <div>
        <div className="flex gap-1">
          {RUNS.map((ok, i) => (
            <span
              key={i}
              className={`h-7 flex-1 rounded-[3px] border ${
                ok ? 'border-success/40 bg-success-soft' : 'border-destructive/50 bg-destructive/15'
              } ${i === RUNS.length - 1 ? 'group-hover:bg-success/40' : ''} transition-colors`}
            />
          ))}
        </div>
        <div className="mt-2 flex justify-between text-[12px] text-foreground-muted">
          <span>Last 14 runs</span>
          <span>Next at 03:00 UTC</span>
        </div>
      </div>
    </div>
  )
}

const CPU = [22, 26, 24, 31, 28, 35, 30, 42, 38, 33, 36, 29, 34, 40, 37, 45, 39, 33, 36, 31]

export function ArtLogs() {
  const w = 240
  const h = 64
  const pts = CPU.map((v, i) => [(i / (CPU.length - 1)) * w, h - (v / 50) * h] as const)
  const line = pts.map(([x, y], i) => `${i ? 'L' : 'M'} ${x.toFixed(1)} ${y.toFixed(1)}`).join(' ')
  return (
    <div className="absolute inset-x-0 bottom-0 top-[42%] flex flex-col justify-center gap-4 px-5">
      <div>
        <div className="mb-1.5 flex justify-between text-[12px] text-foreground-muted">
          <span>CPU</span>
          <span className="text-foreground-light">37%</span>
        </div>
        <svg
          viewBox={`0 0 ${w} ${h}`}
          preserveAspectRatio="none"
          aria-hidden="true"
          className="h-14 w-full overflow-visible"
        >
          <path d={`${line} L ${w} ${h} L 0 ${h} Z`} fill="var(--primary-soft)" />
          <path d={line} fill="none" stroke="var(--primary)" strokeWidth={1.5} vectorEffect="non-scaling-stroke" />
        </svg>
      </div>
      <div className="fade-right overflow-hidden whitespace-nowrap font-mono text-[11.5px] leading-[1.7] text-foreground-lighter">
        <div>12:04:31 [a1b2c3] GET /api/orders 200 12ms</div>
        <div>12:04:32 [d4e5f6] GET /healthz 200 1ms</div>
        <div>12:04:32 [a1b2c3] POST /api/checkout 201 48ms</div>
      </div>
    </div>
  )
}

const YAML_ART: [string, string][] = [
  ['services:', ''],
  ['  - type: ', 'web'],
  ['    name: ', 'shop'],
  ['    numInstances: ', '2'],
  ['  - type: ', 'cron'],
  ['    schedule: ', '"0 3 * * *"'],
  ['databases:', ''],
  ['  - name: ', 'shop-db'],
]

export function ArtBlueprint() {
  return (
    <div className="fade-bottom absolute inset-x-0 bottom-0 top-[40%] px-5">
      <pre className="h-full overflow-hidden rounded-t-lg border border-b-0 border-border bg-surface-75 px-4 pt-3 text-[12px] leading-[1.75] text-foreground-lighter">
        {YAML_ART.map(([k, v], i) => (
          <span key={i} className="block">
            {k}
            <span className="text-foreground transition-colors group-hover:text-primary">{v}</span>
          </span>
        ))}
      </pre>
    </div>
  )
}
