import * as React from 'react'
import { Link } from 'react-router'
import { Plus } from 'lucide-react'

import { CopyButton } from '@/components/patterns/Copy'
import { DatastoreKindIcon, ServiceTypeIcon } from '@/components/patterns/icons'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { listensOnPort, type DatastoreView, type EnvVar, type ServiceView } from '@/lib/api/types'

import { datastoreRef, findReferences, serviceRef, suggestedKey } from './effective'

interface ReferenceHelpProps {
  service: ServiceView
  datastores: DatastoreView[] | undefined
  services: ServiceView[] | undefined
  loading: boolean
  /** The service's variables (editor state) — to suggest free keys and spot existing references. */
  ownVars: EnvVar[]
  /** Append a variable to the editor (unsaved); null while it can't be edited. */
  onAdd: ((key: string, value: string) => void) | null
}

const SYNTAX: { code: string; title: string; more: string }[] = [
  {
    code: '${{datastore.NAME.connectionString}}',
    title: 'Datastore URL on the private network',
    more: 'Also externalConnectionString, host, port, user, password, database.',
  },
  {
    code: '${{service.NAME.hostport}}',
    title: 'host:port of another service',
    more: 'Also host, port, internalUrl and url (public URL).',
  },
  {
    code: '$${{',
    title: 'Escape',
    more: 'Writes a literal ${{ instead of a reference.',
  },
]

/** One resource: icon + name, the reference under it, copy (+ add) on the right. */
function ResourceRow({
  icon,
  name,
  reference,
  action,
}: {
  icon: React.ReactNode
  name: string
  reference: string
  action?: React.ReactNode
}) {
  return (
    <li className="flex items-start gap-2.5 py-2">
      <span aria-hidden="true" className="mt-0.5 shrink-0 text-foreground-lighter [&_svg]:size-3.5">
        {icon}
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate font-mono text-[12.5px] text-foreground">{name}</span>
        <code className="font-mono text-[11.5px] leading-snug break-all text-foreground-lighter">{reference}</code>
        {action}
      </div>
      <CopyButton value={reference} what={`reference ${reference}`} size="icon-tiny" variant="ghost" className="shrink-0" />
    </li>
  )
}

/** Side panel: reference syntax + ready-to-use references to this server's resources. */
export function ReferenceHelp({ service, datastores, services, loading, ownVars, onAdd }: ReferenceHelpProps) {
  const others = (services ?? []).filter((s) => s.id !== service.id && listensOnPort(s.type))
  const takenKeys = React.useMemo(() => ownVars.map((v) => v.key), [ownVars])
  // Which datastore each existing variable already points at.
  const usedBy = React.useMemo(() => {
    const m = new Map<string, string>()
    for (const v of ownVars) {
      for (const r of findReferences(v.value, null)) {
        if (r.kind === 'datastore' && !m.has(r.name)) m.set(r.name, v.key)
      }
    }
    return m
  }, [ownVars])

  return (
    <Card className="gap-0">
      <div className="border-b px-5 py-4">
        <h2 className="text-sm font-medium text-foreground">Reference syntax</h2>
        <p className="mt-0.5 text-[13px] text-foreground-light">
          Values can point at datastores and other services. Ferry resolves them each time a container starts, so rotated
          credentials only need a restart.
        </p>
      </div>
      {/* side by side while the panel sits under the page (below 2xl), stacked in the sidebar */}
      <div className="grid grid-cols-1 md:grid-cols-2 2xl:grid-cols-1">
      <ul className="divide-y md:border-r 2xl:border-r-0">
        {SYNTAX.map((s) => (
          <li key={s.code} className="flex flex-col gap-1.5 px-5 py-3.5">
            <div className="flex min-w-0 items-center gap-1.5 rounded-md border bg-surface-200 py-1 pr-1 pl-2.5">
              <code className="min-w-0 flex-1 font-mono text-[12px] break-all text-foreground">{s.code}</code>
              <CopyButton value={s.code} what="reference syntax" size="icon-tiny" variant="ghost" />
            </div>
            <p className="text-[12.5px] leading-relaxed text-foreground-light">
              <span className="text-foreground">{s.title}.</span> {s.more}
            </p>
          </li>
        ))}
      </ul>

      <div className="flex min-w-0 flex-col border-t md:border-t-0 2xl:border-t">
      <div className="flex flex-col gap-1 px-5 pt-4 pb-2">
        <MonoLabel as="h3">Datastores</MonoLabel>
        {loading && !datastores ? (
          <Skeleton className="my-2 h-8 w-full" />
        ) : !datastores || datastores.length === 0 ? (
          <p className="py-2 text-[12.5px] text-foreground-light">
            None yet.{' '}
            <Link to="/datastores" className="text-primary underline-offset-4 hover:underline">
              Create a datastore
            </Link>
          </p>
        ) : (
          <ul className="flex flex-col">
            {datastores.map((d) => {
              const ref = datastoreRef(d.name)
              const used = usedBy.get(d.name)
              const key = suggestedKey(d.kind, d.name, takenKeys)
              return (
                <ResourceRow
                  key={d.id}
                  icon={<DatastoreKindIcon kind={d.kind} />}
                  name={d.name}
                  reference={ref}
                  action={
                    used ? (
                      <span className="text-[12px] text-foreground-light">
                        Used by <code className="font-mono text-foreground">{used}</code>
                      </span>
                    ) : onAdd ? (
                      <Button
                        size="tiny"
                        variant="link"
                        icon={<Plus />}
                        className="h-5 w-fit text-[12px]"
                        onClick={() => onAdd(key, ref)}
                      >
                        Add as {key}
                      </Button>
                    ) : undefined
                  }
                />
              )
            })}
          </ul>
        )}
      </div>

      <div className="flex flex-col gap-1 border-t px-5 pt-4 pb-2">
        <MonoLabel as="h3">Services</MonoLabel>
        {loading && !services ? (
          <Skeleton className="my-2 h-8 w-full" />
        ) : others.length === 0 ? (
          <p className="py-2 text-[12.5px] text-foreground-light">No other service listens on a port.</p>
        ) : (
          <ul className="flex flex-col">
            {others.slice(0, 8).map((s) => (
              <ResourceRow key={s.id} icon={<ServiceTypeIcon type={s.type} />} name={s.name} reference={serviceRef(s.name)} />
            ))}
          </ul>
        )}
      </div>
      </div>
      </div>
    </Card>
  )
}
