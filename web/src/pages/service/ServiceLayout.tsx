import * as React from 'react'
import { Link, Outlet, useLocation, useParams } from 'react-router'
import { Boxes, History, LayoutDashboard, ListChecks, ScrollText, Settings2, SlidersHorizontal } from 'lucide-react'

import { CopyButton } from '@/components/patterns/Copy'
import { EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { ServiceTypeIcon } from '@/components/patterns/icons'
import { PageContainer } from '@/components/patterns/Page'
import { ServiceStatePill, StatePill } from '@/components/patterns/StatusBadge'
import { InnerMenu, type InnerMenuGroup } from '@/components/shell/InnerMenu'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { ApiError } from '@/lib/api/client'
import { useService } from '@/lib/api/queries'
import { isDeployActive, type ServiceView } from '@/lib/api/types'
import { DEPLOY_STATUS_LABELS, displayUrl, SERVICE_TYPE_LABELS } from '@/lib/format'

import { servicePath, type ServiceOutletContext } from './context'
import { ServiceActions } from './ServiceActions'

function menuGroups(name: string, service: ServiceView | undefined): InnerMenuGroup[] {
  const latest = service?.latest_deploy
  const deploying = latest && isDeployActive(latest.status) ? latest.status : null
  const groups: InnerMenuGroup[] = [
    {
      label: 'General',
      items: [
        { label: 'Overview', to: servicePath(name), end: true, icon: <LayoutDashboard /> },
        {
          label: 'Deploys',
          to: servicePath(name, '/deploys'),
          icon: <History />,
          badge: deploying ? (
            <StatePill tone="info" pulse label={DEPLOY_STATUS_LABELS[deploying]} className="h-[18px] px-1.5 text-[10px]" />
          ) : undefined,
        },
        { label: 'Logs', to: servicePath(name, '/logs'), icon: <ScrollText /> },
        { label: 'Jobs', to: servicePath(name, '/jobs'), icon: <ListChecks /> },
      ],
    },
    {
      label: 'Configuration',
      items: [
        { label: 'Environment', to: servicePath(name, '/environment'), icon: <SlidersHorizontal /> },
        { label: 'Settings', to: servicePath(name, '/settings'), icon: <Settings2 /> },
      ],
    },
  ]
  if (service?.url) {
    groups.push({ label: 'Links', items: [{ label: displayUrl(service.url), to: service.url, external: true }] })
  }
  return groups
}

/** Title block shown above every service page: name + pills, URL + Copy, actions. */
function ServiceHeader({ service }: { service: ServiceView }) {
  return (
    <header className="mb-8 flex flex-col gap-4 border-b pb-6 lg:flex-row lg:items-start lg:justify-between">
      <div className="flex min-w-0 flex-col gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-2.5">
          <ServiceTypeIcon type={service.type} className="size-5 shrink-0 text-foreground-lighter" />
          <h1 className="min-w-0 truncate text-2xl leading-tight font-medium tracking-[-0.01em] text-foreground md:text-[28px]">
            {service.name}
          </h1>
          <ServiceStatePill state={service.state} />
          <Badge variant="outline" className="hidden sm:inline-flex">
            {SERVICE_TYPE_LABELS[service.type]}
          </Badge>
        </div>
        <div className="flex min-w-0 flex-wrap items-center gap-2 text-[15px] text-foreground-light">
          {service.url ? (
            <>
              <a
                href={service.url}
                target="_blank"
                rel="noreferrer"
                className="min-w-0 truncate underline-offset-4 outline-none hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring"
              >
                {displayUrl(service.url)}
              </a>
              <CopyButton value={service.url} label="Copy" what="service URL" />
            </>
          ) : service.type === 'cron_job' ? (
            <>
              <span className="font-mono text-[13px] text-foreground">{service.schedule ?? 'no schedule'}</span>
              <span className="text-[13px] text-foreground-lighter">cron schedule (UTC)</span>
            </>
          ) : service.type === 'private_service' ? (
            <>
              <span className="font-mono text-[13px] text-foreground">
                {service.internal_port ? `${service.internal_host}:${service.internal_port}` : service.internal_host}
              </span>
              <CopyButton
                value={service.internal_port ? `${service.internal_host}:${service.internal_port}` : service.internal_host}
                label="Copy"
                what="private address"
              />
              <span className="text-[13px] text-foreground-lighter">private network only</span>
            </>
          ) : (
            <span className="text-[13px] text-foreground-lighter">
              Runs on the private network as <span className="font-mono text-foreground-light">{service.internal_host}</span>, no port
            </span>
          )}
        </div>
      </div>
      <ServiceActions service={service} />
    </header>
  )
}

function HeaderSkeleton() {
  return (
    <div className="mb-8 flex flex-col gap-3 border-b pb-6" aria-busy="true" aria-label="Loading service">
      <Skeleton className="h-8 w-56" />
      <Skeleton className="h-5 w-72" />
    </div>
  )
}

/**
 * `/services/:name/*` — Studio inner side menu (GENERAL / CONFIGURATION),
 * service header, and the page. Child pages read the loaded service with
 * `useServiceOutlet()` and render their sections directly (no PageContainer).
 */
export function ServiceLayout() {
  const { name = '' } = useParams()
  const { data: service, error, isLoading, refetch, isRefetching } = useService(name)
  const { pathname } = useLocation()
  const scrollRef = React.useRef<HTMLDivElement | null>(null)

  React.useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 })
  }, [pathname])

  const notFound = error instanceof ApiError && error.isNotFound && !service
  const context: ServiceOutletContext | null = service ? { service, name } : null

  return (
    <div className="flex min-h-0 flex-1 flex-col md:flex-row">
      <InnerMenu title={name} label={`Service ${name}`} groups={menuGroups(name, service)} />
      <div ref={scrollRef} className="relative min-h-0 min-w-0 flex-1 overflow-y-auto">
        <PageContainer>
          {notFound ? (
            <EmptyState
              size="lg"
              icon={<Boxes />}
              title={`Service “${name}” not found`}
              description="It may have been deleted or renamed."
              actions={
                <Button asChild variant="primary">
                  <Link to="/services">All services</Link>
                </Button>
              }
            />
          ) : error && !service ? (
            <ErrorState error={error} title="Could not load the service" onRetry={() => void refetch()} retrying={isRefetching} />
          ) : (
            <>
              {service ? <ServiceHeader service={service} /> : <HeaderSkeleton />}
              {context ? (
                <Outlet context={context} />
              ) : (
                isLoading && (
                  <div className="flex flex-col gap-4" aria-hidden="true">
                    <Skeleton className="h-6 w-40" />
                    <Skeleton className="h-40 w-full" />
                    <Skeleton className="h-40 w-full" />
                  </div>
                )
              )}
            </>
          )}
        </PageContainer>
      </div>
    </div>
  )
}
