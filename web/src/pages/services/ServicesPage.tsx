import * as React from 'react'
import { Link, useSearchParams } from 'react-router'
import { Boxes, FileCode2, Plus, SearchX } from 'lucide-react'

import { CodeBlock } from '@/components/patterns/Copy'
import { EmptyState, ErrorState, StaleDataCallout } from '@/components/patterns/EmptyState'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { PageContainer, PageHeader } from '@/components/patterns/Page'
import { RESOURCE_GRID, ResourceCardSkeleton } from '@/components/patterns/ResourceCard'
import { Button } from '@/components/ui/button'
import { useServices } from '@/lib/api/queries'
import type { ServiceState } from '@/lib/api/types'

import { ConnectGitPrompt } from './ConnectGitPrompt'
import { countByState, matchesQuery, parseStatusParam, sortServices } from './lib'
import { ServerPanel } from './ServerPanel'
import { ServiceCard } from './ServiceCard'
import { ServicesTable } from './ServicesTable'
import { ServicesToolbar } from './ServicesToolbar'
import { useNow } from './useNow'
import { useServicesViewPrefs } from './viewPrefs'

/** First-run empty state with a CLI quick start. */
function NoServices() {
  const origin = typeof window !== 'undefined' ? window.location.origin : 'http://127.0.0.1:7878'
  const quickstart = [
    `ferry login --server ${origin} --token <token>`,
    '',
    '# deploy the current directory (creates the service)',
    'ferry up my-app',
    '',
    '# or build from a git repository',
    'ferry create my-app --repo https://github.com/you/app',
  ].join('\n')
  return (
    <EmptyState
      size="lg"
      icon={<Boxes />}
      title="Deploy your first service"
      description="Web services, private services, background workers, cron jobs and static sites run as containers on this server."
      actions={
        <>
          <Button asChild variant="primary" icon={<Plus />}>
            <Link to="/services/new">New service</Link>
          </Button>
          <Button asChild icon={<FileCode2 />}>
            <Link to="/blueprints">Apply a blueprint</Link>
          </Button>
        </>
      }
    >
      <div className="mt-4 flex w-full max-w-xl flex-col gap-2 text-left">
        <MonoLabel>Or from the CLI</MonoLabel>
        <CodeBlock code={quickstart} />
      </div>
    </EmptyState>
  )
}

/** `/services` — Studio "Projects" page: toolbar, cards (or table), server panel. */
export function ServicesPage() {
  const services = useServices()
  const view = useServicesViewPrefs((s) => s.view)
  const sort = useServicesViewPrefs((s) => s.sort)
  const setView = useServicesViewPrefs((s) => s.setView)
  const setSort = useServicesViewPrefs((s) => s.setSort)
  const [params, setParams] = useSearchParams()
  const now = useNow()

  const query = params.get('q') ?? ''
  const states = React.useMemo(() => parseStatusParam(params.get('status')), [params])

  const setParam = (key: string, value: string) =>
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev)
        if (value) next.set(key, value)
        else next.delete(key)
        return next
      },
      // flushSync: re-render before the next click/keystroke, so quick
      // successive changes never build on stale params.
      { replace: true, flushSync: true },
    )
  const setQuery = (q: string) => setParam('q', q)
  const setStates = (s: ServiceState[]) => setParam('status', s.join(','))
  const clearFilters = () =>
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev)
        next.delete('q')
        next.delete('status')
        return next
      },
      { replace: true, flushSync: true },
    )

  const all = services.data
  const counts = React.useMemo(() => countByState(all ?? []), [all])
  const visible = React.useMemo(() => {
    const list = (all ?? []).filter((s) => matchesQuery(s, query) && (states.length === 0 || states.includes(s.state)))
    return sortServices(list, sort)
  }, [all, query, states, sort])

  const loading = services.isPending
  const total = all?.length ?? 0
  const filtered = query.trim() !== '' || states.length > 0

  let content: React.ReactNode
  if (services.isError && !all) {
    content = (
      <ErrorState
        title="Could not load services"
        error={services.error}
        onRetry={() => void services.refetch()}
        retrying={services.isRefetching}
      />
    )
  } else if (!loading && total === 0) {
    content = <NoServices />
  } else if (!loading && visible.length === 0) {
    content = (
      <EmptyState
        icon={<SearchX />}
        title="No services match your filters"
        description={query ? <>Nothing matches “{query.trim()}”{states.length > 0 && ' with the selected status'}.</> : 'No service has the selected status.'}
        actions={<Button onClick={clearFilters}>Clear filters</Button>}
      />
    )
  } else if (view === 'list') {
    content = <ServicesTable services={visible} loading={loading} now={now} />
  } else {
    content = (
      <ul className={RESOURCE_GRID} aria-busy={loading || undefined} aria-label="Services">
        {loading
          ? Array.from({ length: 6 }, (_, i) => <ResourceCardSkeleton key={i} />)
          : visible.map((s) => <ServiceCard key={s.id} service={s} now={now} />)}
      </ul>
    )
  }

  return (
    <PageContainer className="max-w-[1400px]">
      <PageHeader
        title="Services"
        description="Web services, workers, cron jobs and static sites running as containers on this server."
      />
      <div className="grid items-start gap-8 lg:grid-cols-[minmax(0,1fr)_300px] xl:grid-cols-[minmax(0,1fr)_340px]">
        <div className="flex min-w-0 flex-col gap-4">
          <ConnectGitPrompt />
          {(loading || total > 0) && (
            <ServicesToolbar
              query={query}
              onQueryChange={setQuery}
              states={states}
              counts={counts}
              onStatesChange={setStates}
              sort={sort}
              onSortChange={setSort}
              view={view}
              onViewChange={setView}
            />
          )}
          {services.isError && all && (
            <StaleDataCallout
              error={services.error}
              onRetry={() => void services.refetch()}
              retrying={services.isRefetching}
            />
          )}
          {content}
          <p className="sr-only" role="status" aria-live="polite">
            {loading ? 'Loading services' : filtered ? `Showing ${visible.length} of ${total} services` : `${total} services`}
          </p>
        </div>
        <div className="lg:sticky lg:top-6">
          <ServerPanel services={all} loading={loading} />
        </div>
      </div>
    </PageContainer>
  )
}
