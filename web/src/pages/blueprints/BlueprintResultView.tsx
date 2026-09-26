import { Link } from 'react-router'
import { AlertTriangle, Boxes, Braces, Database, History } from 'lucide-react'

import { TableMessageRow } from '@/components/patterns/DataTable'
import { Callout } from '@/components/patterns/EmptyState'
import { PageSection } from '@/components/patterns/Page'
import { DeployStatusBadge } from '@/components/patterns/StatusBadge'
import { Badge } from '@/components/ui/badge'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useDeploy, useServices } from '@/lib/api/queries'
import type { BlueprintResult, Deploy } from '@/lib/api/types'
import { DEPLOY_TRIGGER_LABELS, plural, relativeTime, shortId } from '@/lib/format'
import { cn } from '@/lib/utils'

import { countActions } from './lib'

const RESOURCE_META: Record<string, { label: string; icon: typeof Boxes; path: (name: string) => string }> = {
  service: { label: 'Service', icon: Boxes, path: (n) => `/services/${encodeURIComponent(n)}` },
  datastore: { label: 'Datastore', icon: Database, path: (n) => `/datastores/${encodeURIComponent(n)}` },
  env_group: { label: 'Env group', icon: Braces, path: (n) => `/env-groups/${encodeURIComponent(n)}` },
}

const ACTION_BADGE: Record<string, { variant: 'success' | 'warning' | 'default'; label: string }> = {
  create: { variant: 'success', label: 'Create' },
  update: { variant: 'warning', label: 'Update' },
  unchanged: { variant: 'default', label: 'Unchanged' },
}

function Summary({ result }: { result: BlueprintResult }) {
  const c = countActions(result.actions)
  const applied = !result.dry_run
  const items = [
    { n: c.create, label: applied ? 'created' : 'to create', dot: 'bg-brand' },
    { n: c.update, label: applied ? 'updated' : 'to update', dot: 'bg-warning' },
    { n: c.unchanged, label: 'unchanged', dot: 'bg-foreground-muted' },
  ]
  return (
    <ul className="flex flex-wrap items-center gap-x-5 gap-y-2" aria-label="Summary">
      {items.map((i) => (
        <li key={i.label} className="flex items-baseline gap-2">
          <span className="text-xl text-foreground tabular">{i.n}</span>
          <span className="inline-flex items-center gap-1.5 font-mono text-[11.5px] tracking-[0.06em] text-foreground-lighter uppercase">
            <span aria-hidden="true" className={cn('size-1.5 rounded-full', i.dot)} />
            {i.label}
          </span>
        </li>
      ))}
      {applied && (
        <li className="flex items-baseline gap-2">
          <span className="text-xl text-foreground tabular">{result.deploys.length}</span>
          <span className="font-mono text-[11.5px] tracking-[0.06em] text-foreground-lighter uppercase">
            {result.deploys.length === 1 ? 'deploy queued' : 'deploys queued'}
          </span>
        </li>
      )}
    </ul>
  )
}

function ActionsTable({ result }: { result: BlueprintResult }) {
  return (
    <Table>
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead>Resource</TableHead>
          <TableHead>Name</TableHead>
          <TableHead>Action</TableHead>
          <TableHead className="w-full">Changes</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {result.actions.length === 0 ? (
          <TableMessageRow colSpan={4}>The blueprint declares no resources.</TableMessageRow>
        ) : (
          result.actions.map((a) => {
            const meta = RESOURCE_META[a.resource] ?? { label: a.resource, icon: Boxes, path: () => '' }
            const badge = ACTION_BADGE[a.action] ?? { variant: 'default' as const, label: a.action }
            const exists = !result.dry_run || a.action !== 'create'
            const Icon = meta.icon
            return (
              <TableRow key={`${a.resource}:${a.name}`} className="align-top">
                <TableCell className="align-top text-[13px] text-foreground-light">
                  <span className="inline-flex items-center gap-2">
                    <Icon className="size-4 text-foreground-lighter" aria-hidden="true" />
                    {meta.label}
                  </span>
                </TableCell>
                <TableCell className="align-top font-medium">
                  {exists && meta.path(a.name) ? (
                    <Link
                      to={meta.path(a.name)}
                      className="rounded-sm outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                    >
                      {a.name}
                    </Link>
                  ) : (
                    a.name
                  )}
                </TableCell>
                <TableCell className="align-top">
                  <Badge variant={badge.variant}>{badge.label}</Badge>
                </TableCell>
                <TableCell className="min-w-[240px] align-top whitespace-normal">
                  {a.changes.length > 0 ? (
                    <ul className="flex flex-col gap-1 py-0.5">
                      {a.changes.map((c, i) => (
                        <li
                          key={i}
                          className="font-mono text-[12.5px] leading-relaxed break-words text-foreground-light"
                        >
                          {c}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <span className="text-[13px] text-foreground-lighter">
                      {a.action === 'create' ? `New ${meta.label.toLowerCase()}` : '—'}
                    </span>
                  )}
                </TableCell>
              </TableRow>
            )
          })
        )}
      </TableBody>
    </Table>
  )
}

function DeployRow({ deploy, serviceName }: { deploy: Deploy; serviceName: string | undefined }) {
  const { data } = useDeploy(deploy.id)
  const d = data ?? deploy
  const to = serviceName ? `/services/${encodeURIComponent(serviceName)}/deploys/${encodeURIComponent(d.id)}` : null
  return (
    <TableRow>
      <TableCell className="font-medium">
        {serviceName ? (
          <Link
            to={`/services/${encodeURIComponent(serviceName)}`}
            className="rounded-sm outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
          >
            {serviceName}
          </Link>
        ) : (
          <span className="font-mono text-[13px]">{shortId(d.service_id)}</span>
        )}
      </TableCell>
      <TableCell>
        {to ? (
          <Link
            to={to}
            className="rounded-sm font-mono text-[13px] text-foreground-light outline-none hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring"
          >
            {shortId(d.id)}
          </Link>
        ) : (
          <span className="font-mono text-[13px]">{shortId(d.id)}</span>
        )}
      </TableCell>
      <TableCell className="hidden text-[13px] text-foreground-light sm:table-cell">
        {DEPLOY_TRIGGER_LABELS[d.trigger]}
      </TableCell>
      <TableCell>
        <DeployStatusBadge status={d.status} />
      </TableCell>
      <TableCell className="hidden text-[13px] text-foreground-light md:table-cell">
        {relativeTime(d.created_at)}
      </TableCell>
    </TableRow>
  )
}

function DeploysTable({ deploys }: { deploys: Deploy[] }) {
  const { data: services } = useServices()
  const names = new Map((services ?? []).map((s) => [s.id, s.name]))
  return (
    <Table>
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead>Service</TableHead>
          <TableHead>Deploy</TableHead>
          <TableHead className="hidden sm:table-cell">Trigger</TableHead>
          <TableHead>Status</TableHead>
          <TableHead className="hidden md:table-cell">Created</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {deploys.length === 0 ? (
          <TableMessageRow colSpan={5}>No deploy was needed.</TableMessageRow>
        ) : (
          deploys.map((d) => <DeployRow key={d.id} deploy={d} serviceName={names.get(d.service_id)} />)
        )}
      </TableBody>
    </Table>
  )
}

/** Dry-run plan or apply result: summary, warnings admonition, actions table, queued deploys. */
export function BlueprintResultView({ result, stale }: { result: BlueprintResult; stale: boolean }) {
  const applied = !result.dry_run
  return (
    <>
      <PageSection
        title={applied ? 'Applied' : 'Plan'}
        description={
          applied
            ? 'The changes below were written. Deploys and restarts run in the background.'
            : 'What applying this blueprint would do. Nothing has changed yet.'
        }
        aria-live="polite"
      >
        {stale && (
          <Callout tone="neutral" title="The blueprint changed since this run">
            {applied ? 'Run a dry run to preview the new changes.' : 'Run the dry run again before applying.'}
          </Callout>
        )}
        <Summary result={result} />
        {result.warnings.length > 0 && (
          <Callout tone="warning" icon={<AlertTriangle />} title={plural(result.warnings.length, 'warning')}>
            <ul className="mt-0.5 flex list-disc flex-col gap-1 pl-4">
              {result.warnings.map((w, i) => (
                <li key={i} className="break-words">
                  {w}
                </li>
              ))}
            </ul>
          </Callout>
        )}
        <ActionsTable result={result} />
      </PageSection>
      {applied && (
        <PageSection
          title={
            <span className="inline-flex items-center gap-2">
              <History className="size-5 text-foreground-lighter" aria-hidden="true" /> Queued deploys
            </span>
          }
          description="Follow each deploy’s build log from its page."
        >
          <DeploysTable deploys={result.deploys} />
        </PageSection>
      )}
    </>
  )
}
