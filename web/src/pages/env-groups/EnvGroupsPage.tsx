import * as React from 'react'
import { useNavigate } from 'react-router'
import { Braces, ExternalLink, Link2, MoreVertical, Plus, Search, Trash2, Unlink, X } from 'lucide-react'

import { CodeBlock } from '@/components/patterns/Copy'
import { EmptyState, ErrorState, StaleDataCallout } from '@/components/patterns/EmptyState'
import { SearchInput } from '@/components/patterns/ListToolbar'
import { PageContainer, PageHeader } from '@/components/patterns/Page'
import { CardStatusLine, RESOURCE_GRID, ResourceCard, ResourceCardSkeleton } from '@/components/patterns/ResourceCard'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useEnvGroups } from '@/lib/api/queries'
import type { EnvGroupView } from '@/lib/api/types'
import { dateTime, relativeTime } from '@/lib/format'

import { DeleteEnvGroupDialog } from './DeleteEnvGroupDialog'
import { NewEnvGroupDialog } from './NewEnvGroupDialog'

const MAX_KEYS = 3

function EnvGroupCard({ group, onDelete }: { group: EnvGroupView; onDelete: () => void }) {
  const navigate = useNavigate()
  const to = `/env-groups/${encodeURIComponent(group.name)}`
  const keys = group.vars.map((v) => v.key)
  const linked = group.services
  return (
    <ResourceCard
      href={to}
      name={group.name}
      icon={<Braces />}
      menu={
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-tiny"
              icon={<MoreVertical />}
              aria-label={`Actions for ${group.name}`}
              className="text-foreground-lighter"
            />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuItem onSelect={() => void navigate(to)}>
              <ExternalLink /> Open env group
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" onSelect={onDelete}>
              <Trash2 /> Delete env group…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      }
      subtitle={
        keys.length === 0 ? (
          <span>No variables yet</span>
        ) : (
          <span className="truncate font-mono text-[12.5px]" title={keys.join(', ')}>
            {keys.slice(0, MAX_KEYS).join(' · ')}
            {keys.length > MAX_KEYS && ` +${keys.length - MAX_KEYS}`}
          </span>
        )
      }
      badges={
        <Badge font="mono" shape="square" case="normal">
          {keys.length} {keys.length === 1 ? 'var' : 'vars'}
        </Badge>
      }
      footer={
        <>
          <span className="min-w-0 truncate text-[13px] text-foreground-lighter" title={linked.join(', ') || undefined}>
            {linked.length === 0 ? (
              'Not linked to any service'
            ) : (
              <>
                Used by <span className="font-mono text-foreground-light">{linked.join(', ')}</span>
              </>
            )}
          </span>
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1">
            <CardStatusLine
              icon={linked.length > 0 ? <Link2 /> : <Unlink />}
              ring={linked.length > 0 ? 'border-primary/40 text-primary' : 'border-border-strong text-foreground-lighter'}
            >
              {linked.length === 0 ? 'Unused' : `${linked.length} linked service${linked.length === 1 ? '' : 's'}`}
            </CardStatusLine>
            <time
              dateTime={group.updated_at}
              title={dateTime(group.updated_at)}
              className="text-[12.5px] text-foreground-lighter"
            >
              Updated {relativeTime(group.updated_at)}
            </time>
          </div>
        </>
      }
    />
  )
}

/** `/env-groups` — shared environment variables as Studio project cards. */
export function EnvGroupsPage() {
  const { data, error, isLoading, refetch, isRefetching } = useEnvGroups()
  const [query, setQuery] = React.useState('')
  const [createOpen, setCreateOpen] = React.useState(false)
  const [deleting, setDeleting] = React.useState<EnvGroupView | null>(null)

  const all = React.useMemo(() => [...(data ?? [])].sort((a, b) => a.name.localeCompare(b.name)), [data])
  const q = query.trim().toLowerCase()
  const shown = all.filter(
    (g) =>
      q === '' ||
      g.name.toLowerCase().includes(q) ||
      g.services.some((s) => s.includes(q)) ||
      g.vars.some((v) => v.key.toLowerCase().includes(q)),
  )

  const newButton = (
    <Button variant="primary" icon={<Plus />} onClick={() => setCreateOpen(true)}>
      New env group
    </Button>
  )

  return (
    <PageContainer>
      <PageHeader
        title="Env groups"
        description="Share environment variables across services. Linked services receive the group’s variables at their next deploy or restart."
      />

      {(isLoading || all.length > 0) && (
        <div className="mb-4 flex flex-wrap items-center gap-2">
          <SearchInput
            value={query}
            onChange={setQuery}
            placeholder="Search by name, variable or service"
            label="Search env groups"
          />
          <div className="ml-auto shrink-0">{newButton}</div>
        </div>
      )}

      {error && data && (
        <StaleDataCallout error={error} onRetry={() => void refetch()} retrying={isRefetching} className="mb-4" />
      )}

      {error && !data ? (
        <ErrorState error={error} title="Could not load env groups" onRetry={() => void refetch()} retrying={isRefetching} />
      ) : isLoading ? (
        <ul className={RESOURCE_GRID} aria-busy="true" aria-label="Loading env groups">
          <ResourceCardSkeleton />
          <ResourceCardSkeleton />
          <ResourceCardSkeleton />
        </ul>
      ) : all.length === 0 ? (
        <EmptyState
          size="lg"
          icon={<Braces />}
          title="No env groups yet"
          description="Keep settings used by several services (API keys, feature flags, log levels) in one place."
          actions={newButton}
        >
          <div className="mt-3 w-full max-w-md">
            <CodeBlock code="ferry env-group create shared LOG_LEVEL=info" prompt />
          </div>
        </EmptyState>
      ) : shown.length === 0 ? (
        <EmptyState
          icon={<Search />}
          title="No env groups match your search"
          description={`Nothing matches “${query.trim()}”.`}
          actions={
            <Button icon={<X />} onClick={() => setQuery('')}>
              Clear search
            </Button>
          }
        />
      ) : (
        <ul className={RESOURCE_GRID} aria-label="Env groups">
          {shown.map((g) => (
            <EnvGroupCard key={g.id} group={g} onDelete={() => setDeleting(g)} />
          ))}
        </ul>
      )}

      <p className="sr-only" role="status">
        {isLoading || q === '' ? '' : `Showing ${shown.length} of ${all.length} env groups`}
      </p>

      <NewEnvGroupDialog open={createOpen} onOpenChange={setCreateOpen} />
      {deleting && (
        <DeleteEnvGroupDialog
          group={deleting}
          open
          onOpenChange={(o) => !o && setDeleting(null)}
          onDeleted={() => setDeleting(null)}
        />
      )}
    </PageContainer>
  )
}
