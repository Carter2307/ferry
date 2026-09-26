import * as React from 'react'
import { useNavigate } from 'react-router'
import { Copy, Database, ExternalLink, MoreVertical, Plus, Search, Trash2, X } from 'lucide-react'
import { toast } from 'sonner'

import { CodeBlock } from '@/components/patterns/Copy'
import { EmptyState, ErrorState, StaleDataCallout } from '@/components/patterns/EmptyState'
import { DatastoreKindIcon } from '@/components/patterns/icons'
import { FilterButton, SearchInput } from '@/components/patterns/ListToolbar'
import { PageContainer, PageHeader } from '@/components/patterns/Page'
import { RESOURCE_GRID, ResourceCard, ResourceCardSkeleton } from '@/components/patterns/ResourceCard'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { copyText } from '@/hooks/useCopy'
import { useDatastores } from '@/lib/api/queries'
import { DATASTORE_KINDS, type DatastoreKind, type DatastoreView } from '@/lib/api/types'
import { DATASTORE_KIND_LABELS } from '@/lib/format'

import { DeleteDatastoreDialog } from './DeleteDatastoreDialog'
import { datastoreRef } from './lib'
import { NewDatastoreDialog } from './NewDatastoreDialog'
import { DatastoreStatusLine } from './parts'

async function copyWithToast(value: string, what: string) {
  if (await copyText(value)) toast.success(`Copied ${what}`)
  else toast.error('Could not copy to the clipboard')
}

function DatastoreCard({ ds, onDelete }: { ds: DatastoreView; onDelete: () => void }) {
  const navigate = useNavigate()
  const to = `/datastores/${encodeURIComponent(ds.name)}`
  return (
    <ResourceCard
      href={to}
      name={ds.name}
      icon={<DatastoreKindIcon kind={ds.kind} />}
      menu={
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-tiny"
              icon={<MoreVertical />}
              aria-label={`Actions for ${ds.name}`}
              className="text-foreground-lighter"
            />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-60">
            <DropdownMenuItem onSelect={() => void navigate(to)}>
              <ExternalLink /> Open datastore
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void copyWithToast(datastoreRef(ds.name), 'env reference')}>
              <Copy /> Copy env reference
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void copyWithToast(ds.internal_url, 'connection string')}>
              <Copy /> Copy connection string
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" onSelect={onDelete}>
              <Trash2 /> Delete datastore…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      }
      subtitle={
        <span className="truncate">
          {DATASTORE_KIND_LABELS[ds.kind]}
          {ds.database ? (
            <>
              {' · '}
              <span className="font-mono text-[12.5px]">{ds.database}</span>
            </>
          ) : null}
        </span>
      }
      badges={
        <>
          <Badge font="mono" shape="square">
            {ds.kind}
          </Badge>
          <Badge font="mono" shape="square" variant="outline" case="normal">
            v{ds.version}
          </Badge>
        </>
      }
      footer={
        <>
          <span className="min-w-0 truncate text-[13px] text-foreground-lighter">
            Private{' '}
            <span className="font-mono text-foreground-light">
              {ds.internal_host}:{ds.internal_port}
            </span>
          </span>
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1">
            <DatastoreStatusLine status={ds.status} />
            {ds.host_port && ds.status !== 'failed' && (
              <span className="truncate font-mono text-[12px] text-foreground-lighter" title="Published on the Ferry host">
                127.0.0.1:{ds.host_port}
              </span>
            )}
          </div>
          {ds.status === 'failed' && ds.error && (
            <p className="line-clamp-2 text-[12.5px] break-words text-destructive" title={ds.error}>
              {ds.error}
            </p>
          )}
        </>
      }
    />
  )
}

/** `/datastores` — managed Postgres / Redis as Studio project cards. */
export function DatastoresPage() {
  const { data, error, isLoading, refetch, isRefetching } = useDatastores()
  const [query, setQuery] = React.useState('')
  const [kinds, setKinds] = React.useState<Set<DatastoreKind>>(() => new Set())
  const [createOpen, setCreateOpen] = React.useState(false)
  const [deleting, setDeleting] = React.useState<DatastoreView | null>(null)

  const all = React.useMemo(() => [...(data ?? [])].sort((a, b) => a.name.localeCompare(b.name)), [data])
  const q = query.trim().toLowerCase()
  const shown = all.filter(
    (d) =>
      (kinds.size === 0 || kinds.has(d.kind)) &&
      (q === '' || d.name.includes(q) || d.kind.includes(q) || (d.database ?? '').toLowerCase().includes(q)),
  )
  const filtered = q !== '' || kinds.size > 0
  const clearFilters = () => {
    setQuery('')
    setKinds(new Set())
  }

  const newButton = (
    <Button variant="primary" icon={<Plus />} onClick={() => setCreateOpen(true)}>
      New datastore
    </Button>
  )

  return (
    <PageContainer>
      <PageHeader
        title="Datastores"
        description="Managed PostgreSQL and Redis on the private network, with credentials your services reference by name."
      />

      {(isLoading || all.length > 0) && (
        <div className="mb-4 flex flex-wrap items-center gap-2">
          <SearchInput value={query} onChange={setQuery} placeholder="Search for a datastore" />
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <FilterButton label="Type" selected={[...kinds].map((k) => DATASTORE_KIND_LABELS[k])} />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="w-48">
              <DropdownMenuLabel>Filter by type</DropdownMenuLabel>
              {DATASTORE_KINDS.map((k) => (
                <DropdownMenuCheckboxItem
                  key={k}
                  checked={kinds.has(k)}
                  onSelect={(e) => e.preventDefault()}
                  onCheckedChange={(on) =>
                    setKinds((s) => {
                      const n = new Set(s)
                      if (on) n.add(k)
                      else n.delete(k)
                      return n
                    })
                  }
                >
                  {DATASTORE_KIND_LABELS[k]}
                  <span className="ml-auto font-mono text-[11.5px] text-foreground-lighter tabular">
                    {all.filter((d) => d.kind === k).length}
                  </span>
                </DropdownMenuCheckboxItem>
              ))}
              {kinds.size > 0 && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onSelect={() => setKinds(new Set())}>
                    <X /> Clear filter
                  </DropdownMenuItem>
                </>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
          <div className="ml-auto shrink-0">{newButton}</div>
        </div>
      )}

      {error && data && (
        <StaleDataCallout error={error} onRetry={() => void refetch()} retrying={isRefetching} className="mb-4" />
      )}

      {error && !data ? (
        <ErrorState
          error={error}
          title="Could not load datastores"
          onRetry={() => void refetch()}
          retrying={isRefetching}
        />
      ) : isLoading ? (
        <ul className={RESOURCE_GRID} aria-busy="true" aria-label="Loading datastores">
          <ResourceCardSkeleton />
          <ResourceCardSkeleton />
          <ResourceCardSkeleton />
        </ul>
      ) : all.length === 0 ? (
        <EmptyState
          size="lg"
          icon={<Database />}
          title="No datastores yet"
          description="Create a PostgreSQL database or a Redis instance. Services connect to it over the private network with a reference like ${{datastore.app-db.connectionString}}."
          actions={newButton}
        >
          <div className="mt-3 w-full max-w-md">
            <CodeBlock code="ferry db create app-db --kind postgres" prompt />
          </div>
        </EmptyState>
      ) : shown.length === 0 ? (
        <EmptyState
          icon={<Search />}
          title="No datastores match your filters"
          description={q ? `Nothing matches “${query.trim()}”.` : 'Try another type.'}
          actions={
            <Button icon={<X />} onClick={clearFilters}>
              Clear filters
            </Button>
          }
        />
      ) : (
        <ul className={RESOURCE_GRID} aria-label="Datastores">
          {shown.map((ds) => (
            <DatastoreCard key={ds.id} ds={ds} onDelete={() => setDeleting(ds)} />
          ))}
        </ul>
      )}

      <p className="sr-only" role="status">
        {isLoading ? '' : filtered ? `Showing ${shown.length} of ${all.length} datastores` : ''}
      </p>

      <NewDatastoreDialog open={createOpen} onOpenChange={setCreateOpen} />
      {deleting && (
        <DeleteDatastoreDialog
          datastore={deleting}
          open
          onOpenChange={(o) => !o && setDeleting(null)}
          onDeleted={() => setDeleting(null)}
        />
      )}
    </PageContainer>
  )
}
