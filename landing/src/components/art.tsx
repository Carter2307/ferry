import { Check, Loader, Lock } from 'lucide-react'
import { siPostgresql, siRedis } from 'simple-icons'

import { BrandIcon } from './BrandIcon'

/*
 * Card illustrations: UI fragments and line art in the card's greys, with the
 * blue and green only where they mean something. They fade into the card.
 */

/* The logo's silhouette: the outline of assets/ferry-logo.svg at whole units of its 772 × 582 box, without the sketch's tick marks. */
const SILHOUETTE =
  'M339 150v-73c0-32-6-68 35-71c4-1 15-1 17-2h1c2 1 15 1 18 2c15 2 27 9 32 24c0 3 1 7 2 9l-1 1c0 9 0 29 0 39c0 24 0 48 1 71h51c13 0 30-1 42 2c13 3 25 10 35 20c7 7 13 17 16 26l2 9c5 41 9 83 13 125c15 3 30 8 45 12c10 3 22 6 32 10c3 1 8 4 10 7c3 3 4 8 3 12c-2 12-55 92-67 106c-9 11-19 21-30 29c42-13 77-18 116 6c24 15 31 21 59 28l1 4l-2 2c-1 1-1 0-1 1c-20 18-38 16-62 7c-1-1-3-1-5-2c-8-4-7-2-16-5c-4-1-10-3-14-5c-4 0-8-1-12-1c-26 0-52 14-75 24c-42 19-82 20-123 0c-22-11-45-27-70-28h-1c-38 1-65 29-102 37c-38 9-70 0-104-18c-23-12-45-20-70-17c-4 1-13 5-16 4l3-2l-1 1c-2-1-29 8-34 9c-18 4-37 5-52-7l-1-1v-4c7-5 16-9 23-14c47-32 72-50 130-37c-29-33-56-76-76-115c-11-20 38-29 48-32c14-3 28-7 41-10c1-15 14-124 13-127l-2-1c4 1 4-6 5-9c3-9 8-17 15-24c10-10 22-17 35-20c12-3 30-2 43-2h50zM240 317c3-28 5-57 7-85c3-30 22-25 46-25c9-1 17 0 25 0l47-1c7 0 20 0 26 1l1-1v58v13h-1c-15 3-42 10-57 14c-31 9-64 17-94 26zM468 207l3-1c15 1 34-1 49 1c18 3 17 28 18 41l4 46c0 1 0 3 0 4c1 6 1 12 2 18l-16-4l-1-1c-8-2-17-4-26-7c-6-1-20-5-26-6l-53-14c-9-2-21-6-30-7v-13v-58l76 1z'

/** The Ferry logo's silhouette as a double outline on a construction grid. */
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
      <g transform="translate(56.7 74.7) scale(0.19)" strokeLinejoin="round">
        <path
          d={SILHOUETTE}
          vectorEffect="non-scaling-stroke"
          strokeWidth={6}
          className="stroke-foreground-muted transition-colors duration-300 group-hover:stroke-primary"
        />
        <path d={SILHOUETTE} vectorEffect="non-scaling-stroke" strokeWidth={3.5} stroke="var(--surface-75)" />
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
