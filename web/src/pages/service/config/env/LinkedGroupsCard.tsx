import * as React from 'react'
import { Link } from 'react-router'
import { Braces, Link2, Plus, Unlink } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { ErrorState } from '@/components/patterns/EmptyState'
import { FormCard } from '@/components/patterns/FormCard'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage } from '@/lib/api/client'
import { useLinkEnvGroup, useUnlinkEnvGroup } from '@/lib/api/queries'
import type { EnvGroupView, ServiceView } from '@/lib/api/types'
import { plural } from '@/lib/format'

interface LinkedGroupsCardProps {
  service: ServiceView
  groups: EnvGroupView[] | undefined
  loading: boolean
  error: unknown
  onRetry: () => void
}

function groupPath(name: string): string {
  return `/env-groups/${encodeURIComponent(name)}`
}

/** Env groups linked to the service: link (select), unlink (confirm), open. */
export function LinkedGroupsCard({ service, groups, loading, error, onRetry }: LinkedGroupsCardProps) {
  const name = service.name
  const link = useLinkEnvGroup(name)
  const unlink = useUnlinkEnvGroup(name)
  const [choice, setChoice] = React.useState<string>('')
  const [toUnlink, setToUnlink] = React.useState<string | null>(null)
  const selectId = React.useId()

  const linked = service.env_groups
  const byName = React.useMemo(() => new Map((groups ?? []).map((g) => [g.name, g])), [groups])
  const available = (groups ?? []).filter((g) => !linked.includes(g.name))
  const selected = available.some((g) => g.name === choice) ? choice : ''

  const doLink = () => {
    if (!selected) return
    link.mutate(selected, {
      onSuccess: () => {
        toast.success(`Linked ${selected}`, { description: 'Its variables apply on the next deploy or restart.' })
        setChoice('')
      },
      onError: (e) => toast.error(`Could not link ${selected}`, { description: errorMessage(e) }),
    })
  }

  const linker =
    groups && available.length > 0 ? (
      <div className="flex w-full items-center gap-2 sm:w-auto">
        <label htmlFor={selectId} className="sr-only">
          Environment group to link
        </label>
        <Select value={selected} onValueChange={setChoice}>
          <SelectTrigger id={selectId} size="sm" className="w-full min-w-44 sm:w-52">
            <SelectValue placeholder="Choose a group…" />
          </SelectTrigger>
          <SelectContent position="popper" align="end">
            {available.map((g) => (
              <SelectItem key={g.id} value={g.name}>
                <span className="font-mono">{g.name}</span>
                <span className="text-foreground-lighter">{plural(g.vars.length, 'var')}</span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button icon={<Link2 />} disabled={!selected} loading={link.isPending} onClick={doLink}>
          Link
        </Button>
      </div>
    ) : groups && groups.length === 0 ? (
      <Button asChild size="tiny" icon={<Plus />}>
        <Link to="/env-groups">Create a group</Link>
      </Button>
    ) : undefined

  return (
    <FormCard
      asDiv
      title="Linked environment groups"
      description="Shared variables merged in link order (a later group wins over an earlier one). Changes apply on the next deploy or restart."
      headerActions={linker}
    >
      {loading && linked.length > 0 ? (
        <div className="flex flex-col gap-3 px-5 py-4 md:px-6" aria-busy="true" aria-label="Loading groups">
          {linked.map((g) => (
            <Skeleton key={g} className="h-6 w-56" />
          ))}
        </div>
      ) : error && !groups ? (
        <div className="px-5 py-4 md:px-6">
          <ErrorState error={error} title="Could not load environment groups" onRetry={onRetry} />
        </div>
      ) : linked.length === 0 ? (
        <div className="flex items-center gap-3 px-5 py-5 text-[13px] text-foreground-light md:px-6">
          <Braces className="size-4 shrink-0 text-foreground-lighter" aria-hidden="true" />
          <span>
            No groups linked.{' '}
            {groups && groups.length === 0 ? (
              <>
                Groups share variables between services —{' '}
                <Link to="/env-groups" className="text-primary underline-offset-4 hover:underline">
                  create one
                </Link>
                .
              </>
            ) : (
              'Link a group to share its variables with this service.'
            )}
          </span>
        </div>
      ) : (
        <ul aria-label="Linked groups" className="divide-y">
          {linked.map((g, i) => {
            const group = byName.get(g)
            return (
              <li key={g} className="flex flex-wrap items-center gap-x-3 gap-y-2 px-5 py-3 md:px-6">
                <MonoLabel className="w-5 shrink-0 tabular-nums" aria-label={`Priority ${i + 1}`}>
                  {i + 1}
                </MonoLabel>
                <span
                  aria-hidden="true"
                  className="flex size-7 shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-lighter"
                >
                  <Braces className="size-3.5" />
                </span>
                <div className="flex min-w-0 flex-1 flex-col">
                  <Link
                    to={groupPath(g)}
                    className="w-fit truncate font-mono text-[13px] text-foreground underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    {g}
                  </Link>
                  <span className="text-[12.5px] text-foreground-lighter">
                    {group ? plural(group.vars.length, 'variable') : '—'}
                    {group && group.services.length > 1 && ` · shared with ${plural(group.services.length - 1, 'other service')}`}
                  </span>
                </div>
                <Button
                  size="tiny"
                  icon={<Unlink />}
                  loading={unlink.isPending && unlink.variables === g}
                  onClick={() => setToUnlink(g)}
                >
                  Unlink
                </Button>
              </li>
            )
          })}
        </ul>
      )}
      <ConfirmDialog
        open={toUnlink !== null}
        onOpenChange={(o) => !o && setToUnlink(null)}
        variant="warning"
        title={`Unlink ${toUnlink ?? ''} from ${name}?`}
        description={
          <p>
            Its variables are removed from {name}’s environment at the next deploy or restart. The group itself is not
            deleted.
          </p>
        }
        confirmLabel="Unlink group"
        onConfirm={() => {
          const g = toUnlink
          if (!g) return
          return unlink.mutateAsync(g).then(() => {
            toast.success(`Unlinked ${g}`)
          })
        }}
      />
    </FormCard>
  )
}
