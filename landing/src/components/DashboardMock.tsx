import {
  Boxes,
  Braces,
  ChevronsUpDown,
  Database,
  FileCode2,
  LayoutDashboard,
  ListChecks,
  MoreHorizontal,
  Rocket,
  ScrollText,
  Server,
  Settings2,
  SlidersHorizontal,
  type LucideIcon,
} from 'lucide-react'

import { FerryMark } from './FerryMark'

/*
 * A still of the dashboard (web/): the icon rail, a service's menu and its
 * deploys. Decorative: the real thing ships inside ferryd.
 */

const RAIL: { icon: LucideIcon; active?: boolean }[] = [
  { icon: Boxes, active: true },
  { icon: Database },
  { icon: Braces },
  { icon: FileCode2 },
]

const MENU: { group: string; items: { icon: LucideIcon; label: string; active?: boolean }[] }[] = [
  {
    group: 'General',
    items: [
      { icon: LayoutDashboard, label: 'Overview' },
      { icon: Rocket, label: 'Deploys', active: true },
      { icon: ScrollText, label: 'Logs' },
      { icon: ListChecks, label: 'Jobs' },
    ],
  },
  {
    group: 'Configuration',
    items: [
      { icon: SlidersHorizontal, label: 'Environment' },
      { icon: Settings2, label: 'Settings' },
    ],
  },
]

type Status = 'live' | 'deactivated' | 'failed'

const DEPLOYS: { status: Status; message: string; id: string; trigger: string; took: string; when: string }[] = [
  {
    status: 'live',
    message: 'Add a health check',
    id: 'dep-q4m8x2k7c1',
    trigger: 'upload',
    took: '41s',
    when: '2 min ago',
  },
  {
    status: 'deactivated',
    message: 'Bump dependencies',
    id: 'dep-a41f9c3zw8',
    trigger: 'webhook',
    took: '36s',
    when: '1 h ago',
  },
  { status: 'failed', message: 'Try pnpm', id: 'dep-3b0d11wqk2', trigger: 'webhook', took: '12s', when: '3 h ago' },
  {
    status: 'deactivated',
    message: 'Initial commit',
    id: 'dep-9ac2e0lm5t',
    trigger: 'create',
    took: '58s',
    when: 'Yesterday',
  },
]

const BADGE: Record<Status, { label: string; className: string }> = {
  live: { label: 'Live', className: 'border-success/30 bg-success-soft text-success' },
  deactivated: { label: 'Deactivated', className: 'border-border-strong bg-surface-200 text-foreground-lighter' },
  failed: { label: 'Build failed', className: 'border-destructive/30 bg-destructive/10 text-destructive' },
}

function Badge({ status }: { status: Status }) {
  const b = BADGE[status]
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-[4px] border px-1.5 py-0.5 text-[11.5px] font-medium ${b.className}`}
    >
      {status === 'live' && <span className="size-1.5 rounded-full bg-success" />}
      {b.label}
    </span>
  )
}

export function DashboardMock() {
  return (
    <div aria-hidden="true" className="flex h-full min-h-0 text-[13px]">
      {/* Icon rail */}
      <div className="flex w-12 shrink-0 flex-col items-center gap-1 border-r border-border bg-surface-75 py-3 max-sm:hidden">
        <FerryMark className="mb-3 size-5" />
        {RAIL.map(({ icon: Icon, active }, i) => (
          <span
            key={i}
            className={`grid size-8 place-items-center rounded-md ${active ? 'bg-surface-300 text-foreground' : 'text-foreground-muted'}`}
          >
            <Icon className="size-4" />
          </span>
        ))}
        <span className="mt-auto grid size-8 place-items-center rounded-md text-foreground-muted">
          <Server className="size-4" />
        </span>
      </div>

      {/* Service menu */}
      <div className="w-52 shrink-0 border-r border-border bg-surface-75 max-md:hidden">
        <div className="flex h-12 items-center justify-between border-b border-border px-4 font-medium">
          my-app
          <ChevronsUpDown className="size-3.5 text-foreground-muted" />
        </div>
        <div className="space-y-5 p-3">
          {MENU.map((g) => (
            <div key={g.group}>
              <div className="px-2 pb-1.5 text-[12px] text-foreground-muted">{g.group}</div>
              {g.items.map(({ icon: Icon, label, active }) => (
                <div
                  key={label}
                  className={`flex items-center gap-2.5 rounded-md px-2 py-1.5 ${
                    active ? 'bg-surface-300 text-foreground' : 'text-foreground-light'
                  }`}
                >
                  <Icon className="size-4 text-foreground-lighter" />
                  {label}
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>

      {/* Deploys */}
      <div className="min-w-0 flex-1">
        <div className="flex h-12 items-center gap-2 border-b border-border px-5 text-foreground-lighter">
          <span>Services</span>
          <span className="text-foreground-muted">/</span>
          <span className="text-foreground">my-app</span>
        </div>
        <div className="p-5 sm:p-6">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <div className="flex items-center gap-2.5">
                <span className="font-display text-lg font-medium text-foreground">Deploys</span>
                <Badge status="live" />
              </div>
              <div className="mt-1 text-foreground-lighter">
                Web service at{' '}
                <span className="text-foreground-light underline decoration-border-stronger underline-offset-2">
                  my-app.example.com
                </span>
              </div>
            </div>
            <span className="btn btn-secondary btn-sm pointer-events-none">Manual deploy</span>
          </div>

          <div className="mt-5 overflow-hidden rounded-lg border border-border bg-surface-100">
            <div className="grid grid-cols-[7rem_minmax(0,1fr)_5rem] gap-3 border-b border-border bg-surface-75 px-4 py-2 text-[12px] text-foreground-lighter sm:grid-cols-[7rem_minmax(0,1fr)_5rem_4rem_6rem_1.5rem]">
              <span>Status</span>
              <span>Deploy</span>
              <span>Trigger</span>
              <span className="max-sm:hidden">Took</span>
              <span className="max-sm:hidden">Started</span>
              <span className="max-sm:hidden" />
            </div>
            {DEPLOYS.map((d) => (
              <div
                key={d.id}
                className="grid grid-cols-[7rem_minmax(0,1fr)_5rem] items-center gap-3 border-b border-border px-4 py-3 last:border-b-0 sm:grid-cols-[7rem_minmax(0,1fr)_5rem_4rem_6rem_1.5rem]"
              >
                <span>
                  <Badge status={d.status} />
                </span>
                <span className="min-w-0">
                  <span className="block truncate text-foreground">{d.message}</span>
                  <span className="block truncate font-mono text-[11.5px] text-foreground-muted">{d.id}</span>
                </span>
                <span className="text-foreground-light">{d.trigger}</span>
                <span className="text-foreground-light tabular-nums max-sm:hidden">{d.took}</span>
                <span className="text-foreground-lighter max-sm:hidden">{d.when}</span>
                <MoreHorizontal className="size-4 text-foreground-muted max-sm:hidden" />
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}
