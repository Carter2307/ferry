import * as React from 'react'
import { Link } from 'react-router'
import { Braces, CircleAlert } from 'lucide-react'

import { DatastoreKindIcon, ServiceTypeIcon } from '@/components/patterns/icons'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { StatusDot } from '@/components/patterns/StatusBadge'
import { DATASTORE_STATUS_TONE, SERVICE_STATE_TONE, type StatusTone } from '@/components/patterns/status-tones'
import { Skeleton } from '@/components/ui/skeleton'
import { useDatastores, useEnvGroups, useServiceEnv, useServices } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'
import { DATASTORE_KIND_LABELS, displayUrl, plural, SERVICE_TYPE_LABELS } from '@/lib/format'
import { cn } from '@/lib/utils'

import { servicePath } from '../context'
import { referencedResources } from './env-refs'
import { privateAddress } from './utils'

interface TopologyNode {
  id: string
  kind: 'env_group' | 'datastore' | 'service'
  name: string
  to: string | null
  icon: React.ReactNode
  subtitle: string
  meta?: string
  tone: StatusTone | null
  /** Referenced but not found on the server. */
  missing: boolean
}

function useTopologyNodes(service: ServiceView) {
  const env = useServiceEnv(service.name)
  const datastores = useDatastores()
  const groups = useEnvGroups()
  const services = useServices()

  const nodes = React.useMemo<TopologyNode[]>(() => {
    const out: TopologyNode[] = []
    for (const g of service.env_groups) {
      const group = groups.data?.find((x) => x.name === g || x.id === g)
      out.push({
        id: `env_group:${g}`,
        kind: 'env_group',
        name: group?.name ?? g,
        to: `/env-groups/${encodeURIComponent(group?.name ?? g)}`,
        icon: <Braces />,
        subtitle: 'Environment group',
        meta: group ? plural(group.vars.length, 'variable') : undefined,
        tone: null,
        missing: groups.isSuccess && !group,
      })
    }
    for (const ref of referencedResources(env.data ?? [])) {
      const via = `via ${ref.keys.join(', ')}`
      if (ref.kind === 'datastore') {
        const ds = datastores.data?.find((d) => d.name === ref.name)
        out.push({
          id: `datastore:${ref.name}`,
          kind: 'datastore',
          name: ref.name,
          to: ds ? `/datastores/${encodeURIComponent(ds.name)}` : null,
          icon: <DatastoreKindIcon kind={ds?.kind ?? 'postgres'} />,
          subtitle: ds ? `${DATASTORE_KIND_LABELS[ds.kind]} ${ds.version}` : 'Datastore',
          meta: via,
          tone: ds ? DATASTORE_STATUS_TONE[ds.status].tone : null,
          missing: datastores.isSuccess && !ds,
        })
      } else if (ref.name !== service.name) {
        const svc = services.data?.find((s) => s.name === ref.name)
        out.push({
          id: `service:${ref.name}`,
          kind: 'service',
          name: ref.name,
          to: svc ? servicePath(svc.name) : null,
          icon: <ServiceTypeIcon type={svc?.type ?? 'web_service'} />,
          subtitle: svc ? SERVICE_TYPE_LABELS[svc.type] : 'Service',
          meta: via,
          tone: svc ? SERVICE_STATE_TONE[svc.state].tone : null,
          missing: services.isSuccess && !svc,
        })
      }
    }
    return out
  }, [
    service.env_groups,
    service.name,
    env.data,
    datastores.data,
    datastores.isSuccess,
    groups.data,
    groups.isSuccess,
    services.data,
    services.isSuccess,
  ])

  return { nodes, loading: env.isLoading, error: env.error }
}

interface Edge {
  id: string
  d: string
  from: { x: number; y: number }
  to: { x: number; y: number }
  missing: boolean
}

/** Measure node positions and draw curved connectors from the service to each dependency. */
function useEdges(
  containerRef: React.RefObject<HTMLDivElement | null>,
  sourceRef: React.RefObject<HTMLDivElement | null>,
  nodes: TopologyNode[],
): Edge[] {
  const [edges, setEdges] = React.useState<Edge[]>([])
  const signature = nodes.map((n) => `${n.id}:${n.missing}`).join('|')

  React.useLayoutEffect(() => {
    const container = containerRef.current
    const source = sourceRef.current
    if (!container || !source) return
    const compute = () => {
      const c = container.getBoundingClientRect()
      const s = source.getBoundingClientRect()
      const next: Edge[] = []
      container.querySelectorAll<HTMLElement>('[data-topology-node]').forEach((el) => {
        const r = el.getBoundingClientRect()
        const id = el.dataset.topologyNode ?? ''
        const missing = el.dataset.missing === 'true'
        const horizontal = r.left >= s.right - 1
        let from: { x: number; y: number }
        const to = { x: r.left - c.left, y: r.top - c.top + r.height / 2 }
        let d: string
        if (horizontal) {
          from = { x: s.right - c.left, y: s.top - c.top + s.height / 2 }
          const mx = (from.x + to.x) / 2
          d = `M ${from.x} ${from.y} C ${mx} ${from.y}, ${mx} ${to.y}, ${to.x} ${to.y}`
        } else {
          // stacked (narrow): drop from the service's icon, curve into the node's left edge
          from = { x: s.left - c.left + 32, y: s.bottom - c.top }
          d = `M ${from.x} ${from.y} C ${from.x} ${to.y}, ${from.x} ${to.y}, ${to.x} ${to.y}`
        }
        next.push({ id, d, from, to, missing })
      })
      setEdges((prev) => (JSON.stringify(prev) === JSON.stringify(next) ? prev : next))
    }
    compute()
    const ro = new ResizeObserver(compute)
    ro.observe(container)
    return () => ro.disconnect()
  }, [containerRef, sourceRef, signature])

  return edges
}

function NodeCard({
  node,
  className,
  ...props
}: { node: TopologyNode; className?: string } & React.HTMLAttributes<HTMLElement>) {
  const body = (
    <>
      <span
        aria-hidden="true"
        className={cn(
          'flex size-8 shrink-0 items-center justify-center rounded-md border bg-surface-200 text-foreground-light [&_svg]:size-4',
          node.missing && 'border-destructive-border text-destructive',
        )}
      >
        {node.missing ? <CircleAlert /> : node.icon}
      </span>
      <span className="flex min-w-0 flex-col">
        <span className="flex min-w-0 items-center gap-1.5">
          <span className="truncate text-[13px] font-medium text-foreground">{node.name}</span>
          {node.tone && <StatusDot tone={node.tone} />}
        </span>
        <span className={cn('truncate text-[12px]', node.missing ? 'text-destructive' : 'text-foreground-lighter')}>
          {node.missing ? `${node.subtitle} not found` : node.subtitle}
        </span>
        {node.meta && (
          <span className="truncate font-mono text-[10.5px] tracking-[0.02em] text-foreground-lighter" title={node.meta}>
            {node.meta}
          </span>
        )}
      </span>
    </>
  )
  const cls = cn(
    'relative flex w-full min-w-0 items-center gap-2.5 rounded-lg border bg-surface-100 px-3 py-2.5 text-left shadow-card',
    node.missing && 'border-dashed border-destructive-border',
    className,
  )
  return node.to && !node.missing ? (
    <Link
      to={node.to}
      className={cn(
        cls,
        'transition-colors outline-none hover:border-border-stronger focus-visible:ring-2 focus-visible:ring-ring',
      )}
      {...props}
    >
      {body}
    </Link>
  ) : (
    <div className={cls} {...props}>
      {body}
    </div>
  )
}

/**
 * Dotted-grid architecture canvas (Studio project home): the service node and
 * what it depends on — linked env groups plus the datastores and services its
 * variables reference with `${{datastore.X.…}}` / `${{service.X.…}}`.
 */
export function ServiceTopology({ service }: { service: ServiceView }) {
  const { nodes, loading, error } = useTopologyNodes(service)
  const containerRef = React.useRef<HTMLDivElement | null>(null)
  const sourceRef = React.useRef<HTMLDivElement | null>(null)
  const edges = useEdges(containerRef, sourceRef, nodes)
  const tone = SERVICE_STATE_TONE[service.state].tone

  return (
    <figure
      aria-label={`Connections of ${service.name}`}
      className="relative flex min-h-[260px] flex-col overflow-hidden rounded-lg border bg-background bg-dot-grid @container"
    >
      <div
        ref={containerRef}
        className={cn(
          'relative flex flex-1 flex-col gap-8 p-5',
          nodes.length > 0
            ? '@min-[400px]:flex-row @min-[400px]:items-center @min-[400px]:justify-center @min-[400px]:gap-10 @min-[640px]:gap-24'
            : 'items-center justify-center',
        )}
      >
        <svg aria-hidden="true" className="pointer-events-none absolute inset-0 size-full overflow-visible">
          {edges.map((e) => (
            <g key={e.id} className={e.missing ? 'text-destructive/60' : 'text-foreground-muted'}>
              <path
                d={e.d}
                fill="none"
                stroke="currentColor"
                strokeWidth={1.25}
                strokeDasharray={e.missing ? '4 4' : undefined}
              />
              <circle cx={e.from.x} cy={e.from.y} r={2.5} fill="currentColor" />
              <circle cx={e.to.x} cy={e.to.y} r={2.5} fill="currentColor" />
            </g>
          ))}
        </svg>

        {/* the service */}
        <div
          ref={sourceRef}
          className={cn(
            'relative z-10 flex w-full max-w-[250px] min-w-0 items-center gap-3 rounded-lg border border-border-strong bg-surface-100 px-3.5 py-3 shadow-card',
            nodes.length > 0
              ? 'self-start @min-[400px]:w-auto @min-[400px]:flex-[0_1_230px] @min-[400px]:self-center'
              : '',
          )}
        >
          <span
            aria-hidden="true"
            className="flex size-9 shrink-0 items-center justify-center rounded-md border border-primary/30 bg-primary-soft text-primary [&_svg]:size-[18px]"
          >
            <ServiceTypeIcon type={service.type} />
          </span>
          <span className="flex min-w-0 flex-col">
            <span className="flex min-w-0 items-center gap-1.5">
              <span className="truncate text-sm font-medium text-foreground">{service.name}</span>
              <StatusDot tone={tone} pulse={service.state === 'deploying'} />
            </span>
            <span className="truncate text-[12px] text-foreground-lighter">
              {service.url ? displayUrl(service.url) : SERVICE_TYPE_LABELS[service.type]}
            </span>
            <span className="truncate font-mono text-[10.5px] text-foreground-lighter">
              {privateAddress(service)} · {service.instances}×
            </span>
          </span>
        </div>

        {/* dependencies */}
        {loading ? (
          <div
            className="flex w-full min-w-0 flex-col gap-3 pl-12 @min-[400px]:w-auto @min-[400px]:flex-[0_1_220px] @min-[400px]:pl-0"
            aria-hidden="true"
          >
            <Skeleton className="h-14 w-full rounded-lg" />
            <Skeleton className="h-14 w-full rounded-lg" />
          </div>
        ) : nodes.length > 0 ? (
          <ul
            className="relative z-10 flex w-full min-w-0 flex-col gap-3 pl-12 @min-[400px]:w-auto @min-[400px]:flex-[0_1_220px] @min-[400px]:pl-0"
            aria-label="Depends on"
          >
            {nodes.map((n) => (
              <li key={n.id}>
                <NodeCard node={n} data-topology-node={n.id} data-missing={n.missing ? 'true' : 'false'} />
              </li>
            ))}
          </ul>
        ) : null}
      </div>
      <figcaption className="flex flex-wrap items-center justify-between gap-2 border-t bg-surface-100/70 px-4 py-2 backdrop-blur-[2px]">
        <MonoLabel>{nodes.length > 0 ? `Depends on ${nodes.length}` : 'No connections'}</MonoLabel>
        <span className="text-[12px] text-foreground-lighter">
          {error ? (
            <span className="text-destructive">Could not read the environment</span>
          ) : nodes.length > 0 ? (
            'Env groups and ${{…}} references'
          ) : (
            <>
              Reference a datastore with{' '}
              <code className="font-mono text-foreground-light">{'${{datastore.<name>.connectionString}}'}</code>
            </>
          )}
        </span>
      </figcaption>
    </figure>
  )
}
