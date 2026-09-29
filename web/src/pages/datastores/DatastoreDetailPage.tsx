import * as React from 'react'
import { Link, useNavigate, useParams } from 'react-router'
import { ArrowLeft, Database, Loader2, Trash2, XCircle } from 'lucide-react'

import { CodeBlock, CopyButton, CopyField, SecretField } from '@/components/patterns/Copy'
import { Callout, EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { ServiceTypeIcon } from '@/components/patterns/icons'
import { PageContainer, PageHeader, PageSection } from '@/components/patterns/Page'
import { DatastoreStatusBadge, ServiceStatePill } from '@/components/patterns/StatusBadge'
import { TableErrorRow, TableMessageRow, TableSkeletonRows } from '@/components/patterns/DataTable'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ApiError } from '@/lib/api/client'
import { useDatastore } from '@/lib/api/queries'
import type { DatastoreView } from '@/lib/api/types'
import { DATASTORE_KIND_LABELS, dateTime, relativeTime } from '@/lib/format'

import { DeleteDatastoreDialog } from './DeleteDatastoreDialog'
import { DATASTORE_PROPERTIES, datastoreRef, maskPassword, suggestedEnvKey } from './lib'
import { KindIconBox } from './parts'
import { DatastoreResourcesSection } from './ResourcesSection'
import { useDatastoreReferences, type DatastoreReference } from './useDatastoreReferences'

function BackLink() {
  return (
    <Link
      to="/datastores"
      className="inline-flex items-center gap-1.5 rounded-sm text-foreground-lighter outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
    >
      <ArrowLeft className="size-3.5" aria-hidden="true" /> Datastores
    </Link>
  )
}

function StatusCallout({ ds }: { ds: DatastoreView }) {
  if (ds.status === 'creating') {
    return (
      <Callout tone="info" icon={<Loader2 className="animate-spin" />} title="Provisioning…" className="mb-8">
        Ferry is pulling the image and waiting for {DATASTORE_KIND_LABELS[ds.kind]} to accept connections. This page
        updates on its own.
      </Callout>
    )
  }
  if (ds.status === 'failed') {
    return (
      <Callout tone="destructive" icon={<XCircle />} title="Provisioning failed" className="mb-8">
        <p className="break-words">{ds.error ?? 'The container did not become ready.'}</p>
        <p className="mt-1">Delete this datastore and create it again once the cause is fixed.</p>
      </Callout>
    )
  }
  return null
}

function ConnectionCard({ ds }: { ds: DatastoreView }) {
  const internalAddress = `${ds.internal_host}:${ds.internal_port}`
  return (
    <FormCard asDiv>
      <FormRow label="Status" description={`Updated ${relativeTime(ds.updated_at)}`}>
        <div className="flex min-h-[34px] items-center">
          <DatastoreStatusBadge status={ds.status} />
        </div>
      </FormRow>
      <FormRow label="Internal address" description="Host and port on the private network, for services and jobs.">
        <CopyField value={internalAddress} what="internal address" aria-label="Internal address" />
      </FormRow>
      <FormRow
        label="Internal connection string"
        description="Connect from services on the private network. Prefer the env reference below, so credentials never live in your settings."
      >
        <SecretField
          value={ds.internal_url}
          mask={maskPassword(ds.internal_url, ds.password)}
          what="internal connection string"
          aria-label="Internal connection string"
        />
      </FormRow>
      <FormRow
        label="External connection string"
        description={
          ds.host_port
            ? `Published on the Ferry host at 127.0.0.1:${ds.host_port}, for local tools such as psql or a GUI client.`
            : 'Not published on the host.'
        }
      >
        {ds.external_url ? (
          <SecretField
            value={ds.external_url}
            mask={maskPassword(ds.external_url, ds.password)}
            what="external connection string"
            aria-label="External connection string"
          />
        ) : (
          <p className="flex min-h-[34px] items-center text-sm text-foreground-lighter">Not available</p>
        )}
      </FormRow>
      <FormRow label="Username">
        <CopyField value={ds.username} what="username" aria-label="Username" />
      </FormRow>
      <FormRow label="Password" description="Generated when the datastore was created.">
        <SecretField value={ds.password} what="password" aria-label="Password" />
      </FormRow>
      {ds.kind === 'postgres' && (
        <FormRow label="Database">
          <CopyField value={ds.database ?? ''} what="database name" aria-label="Database" />
        </FormRow>
      )}
    </FormCard>
  )
}

function UseInServiceCard({ ds }: { ds: DatastoreView }) {
  const snippet = `${suggestedEnvKey(ds.kind)}=${datastoreRef(ds.name)}`
  const properties = DATASTORE_PROPERTIES.filter((p) => ds.kind === 'postgres' || !p.postgresOnly)
  return (
    <FormCard asDiv>
      <FormRow
        layout="vertical"
        label="Environment variable"
        description={
          <>
            Add this to a service’s environment (or an env group). Ferry resolves the reference when the container
            starts, on the private network.
          </>
        }
      >
        <CodeBlock code={snippet} />
      </FormRow>
      <div className="px-5 py-5 md:px-6">
        <p className="mb-3 text-sm font-medium text-foreground">Other properties</p>
        <Table containerClassName="shadow-none">
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead>Property</TableHead>
              <TableHead className="hidden sm:table-cell">Description</TableHead>
              <TableHead className="w-10">
                <span className="sr-only">Copy</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {properties.map((p) => (
              <TableRow key={p.property}>
                <TableCell className="font-mono text-[12.5px]">
                  <span className="hidden text-foreground-lighter md:inline">
                    {'${{'}datastore.{ds.name}.
                  </span>
                  {p.property}
                  <span className="hidden text-foreground-lighter md:inline">{'}}'}</span>
                </TableCell>
                <TableCell className="hidden text-[13px] text-foreground-light sm:table-cell">
                  {p.description}
                </TableCell>
                <TableCell className="py-1 text-right">
                  <CopyButton value={datastoreRef(ds.name, p.property)} what={`${p.property} reference`} />
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </FormCard>
  )
}

function ReferencesTable({ refs, loading, error }: { refs: DatastoreReference[]; loading: boolean; error: unknown }) {
  return (
    <Table>
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead>Service</TableHead>
          <TableHead>Variables</TableHead>
          <TableHead className="hidden md:table-cell">Defined in</TableHead>
          <TableHead className="hidden sm:table-cell">State</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {loading && refs.length === 0 ? (
          <TableSkeletonRows columns={4} rows={2} />
        ) : error && refs.length === 0 ? (
          <TableErrorRow colSpan={4} error={error} />
        ) : refs.length === 0 ? (
          <TableMessageRow colSpan={4}>No service references this datastore yet.</TableMessageRow>
        ) : (
          refs.map((r) => (
            <TableRow key={`${r.service.id}-${r.group ?? ''}`}>
              <TableCell>
                <Link
                  to={`/services/${encodeURIComponent(r.service.name)}/environment`}
                  className="inline-flex items-center gap-2 rounded-sm font-medium outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <ServiceTypeIcon type={r.service.type} className="size-4 text-foreground-lighter" />
                  {r.service.name}
                </Link>
              </TableCell>
              <TableCell className="font-mono text-[12.5px] whitespace-normal text-foreground-light">
                {r.keys.join(', ')}
              </TableCell>
              <TableCell className="hidden text-[13px] text-foreground-light md:table-cell">
                {r.group ? (
                  <Link
                    to={`/env-groups/${encodeURIComponent(r.group)}`}
                    className="rounded-sm underline-offset-4 outline-none hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    Env group {r.group}
                  </Link>
                ) : (
                  'Service environment'
                )}
              </TableCell>
              <TableCell className="hidden sm:table-cell">
                <ServiceStatePill state={r.service.state} />
              </TableCell>
            </TableRow>
          ))
        )}
      </TableBody>
    </Table>
  )
}

function DetailSkeleton() {
  return (
    <div aria-busy="true" aria-label="Loading datastore" className="flex flex-col gap-8">
      <div className="flex items-center gap-4">
        <Skeleton className="size-11 rounded-lg" />
        <div className="flex flex-col gap-2">
          <Skeleton className="h-7 w-48" />
          <Skeleton className="h-4 w-64" />
        </div>
      </div>
      <Skeleton className="h-6 w-40" />
      <Card className="divide-y">
        {Array.from({ length: 5 }, (_, i) => (
          <div key={i} className="grid gap-3 px-6 py-5 md:grid-cols-2 md:gap-8">
            <Skeleton className="h-4 w-36" />
            <Skeleton className="h-[34px] w-full" />
          </div>
        ))}
      </Card>
    </div>
  )
}

/** `/datastores/:name` — Studio settings-style rows: status, connection info, resource limits, references, delete. */
export function DatastoreDetailPage() {
  const { name = '' } = useParams()
  const navigate = useNavigate()
  const { data: ds, error, refetch, isRefetching } = useDatastore(name)
  const references = useDatastoreReferences(ds ? ds.name : undefined)
  const [deleteOpen, setDeleteOpen] = React.useState(false)

  const notFound = error instanceof ApiError && error.isNotFound && !ds

  if (notFound) {
    return (
      <PageContainer size="narrow">
        <EmptyState
          size="lg"
          icon={<Database />}
          title={`Datastore “${name}” not found`}
          description="It may have been deleted."
          actions={
            <Button asChild variant="primary">
              <Link to="/datastores">All datastores</Link>
            </Button>
          }
        />
      </PageContainer>
    )
  }

  return (
    <PageContainer size="narrow">
      {error && !ds ? (
        <>
          <div className="mb-6 text-[13px]">
            <BackLink />
          </div>
          <ErrorState
            error={error}
            title="Could not load the datastore"
            onRetry={() => void refetch()}
            retrying={isRefetching}
          />
        </>
      ) : !ds ? (
        <DetailSkeleton />
      ) : (
        <>
          <PageHeader
            eyebrow={<BackLink />}
            title={
              <span className="flex min-w-0 items-center gap-3">
                <KindIconBox kind={ds.kind} size="lg" />
                <span className="truncate">{ds.name}</span>
              </span>
            }
            badges={
              <>
                <Badge variant="outline" font="mono" case="normal">
                  {DATASTORE_KIND_LABELS[ds.kind]} {ds.version}
                </Badge>
                <DatastoreStatusBadge status={ds.status} />
              </>
            }
            description={
              <span title={dateTime(ds.created_at)}>
                Created {relativeTime(ds.created_at)} · <span className="font-mono text-[13px]">{ds.id}</span>
              </span>
            }
          />

          <StatusCallout ds={ds} />

          <PageSection title="Connection" description="Credentials and addresses for this datastore.">
            <ConnectionCard ds={ds} />
          </PageSection>

          <DatastoreResourcesSection key={ds.id} ds={ds} />

          <PageSection
            title="Use in a service"
            description="Reference the datastore from environment variables instead of copying secrets."
          >
            <UseInServiceCard ds={ds} />
          </PageSection>

          <PageSection
            title="Referenced by"
            description="Services whose environment (or a linked env group) references this datastore."
          >
            <ReferencesTable refs={references.refs} loading={references.loading} error={references.error} />
          </PageSection>

          <PageSection title="Danger zone">
            <Card className="border-destructive-border">
              <div className="flex flex-col gap-4 px-5 py-5 sm:flex-row sm:items-center sm:justify-between md:px-6">
                <div className="flex min-w-0 flex-col gap-1">
                  <p className="text-sm font-medium text-foreground">Delete this datastore</p>
                  <p className="text-[13px] text-foreground-light">
                    Stops the container and deletes its volume. Its data can’t be recovered.
                  </p>
                </div>
                <Button variant="danger" icon={<Trash2 />} onClick={() => setDeleteOpen(true)} className="shrink-0">
                  Delete datastore
                </Button>
              </div>
            </Card>
          </PageSection>

          <DeleteDatastoreDialog
            datastore={ds}
            open={deleteOpen}
            onOpenChange={setDeleteOpen}
            references={references.refs}
            onDeleted={() => void navigate('/datastores', { replace: true })}
          />
        </>
      )}
    </PageContainer>
  )
}
