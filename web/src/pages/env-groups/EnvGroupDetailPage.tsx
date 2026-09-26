import * as React from 'react'
import { Link, useNavigate, useParams } from 'react-router'
import { AlertTriangle, ArrowLeft, Braces, Link2, Trash2, Unlink } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { TableMessageRow, TableSkeletonRows } from '@/components/patterns/DataTable'
import { Callout, EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { FormCard } from '@/components/patterns/FormCard'
import { ServiceTypeIcon } from '@/components/patterns/icons'
import { KeyValueEditor } from '@/components/patterns/KeyValueEditor'
import { useKeyValueRows } from '@/components/patterns/kv-rows'
import { PageContainer, PageHeader, PageSection } from '@/components/patterns/Page'
import { ServiceStatePill } from '@/components/patterns/StatusBadge'
import { useUnsavedChangesGuard } from '@/components/patterns/useUnsavedChangesGuard'
import { VariablesFooter } from '@/components/patterns/VariablesFooter'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Hint } from '@/components/ui/tooltip'
import { ApiError, errorMessage } from '@/lib/api/client'
import { useEnvGroup, useReplaceEnvGroupEnv, useServices } from '@/lib/api/queries'
import type { EnvGroupView, EnvVar, ServiceView } from '@/lib/api/types'
import { dateTime, plural, relativeTime, SERVICE_TYPE_LABELS } from '@/lib/format'

import { DeleteEnvGroupDialog } from './DeleteEnvGroupDialog'
import { useServiceGroupLink } from './lib'

const varsKey = (vars: EnvVar[]) => JSON.stringify(vars)

function BackLink() {
  return (
    <Link
      to="/env-groups"
      className="inline-flex items-center gap-1.5 rounded-sm text-foreground-lighter outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
    >
      <ArrowLeft className="size-3.5" aria-hidden="true" /> Env groups
    </Link>
  )
}

/** Variables card: KeyValueEditor + the shared Cancel · Save only · Save & restart footer. */
function VariablesCard({ group, onDirtyChange }: { group: EnvGroupView; onDirtyChange: (dirty: boolean) => void }) {
  const kv = useKeyValueRows(group.vars)
  const replace = useReplaceEnvGroupEnv(group.name)
  const [loaded, setLoaded] = React.useState({ id: group.id, key: varsKey(group.vars) })
  const [saveError, setSaveError] = React.useState<string | null>(null)
  const [pendingAction, setPendingAction] = React.useState<'save' | 'restart' | null>(null)
  React.useEffect(() => onDirtyChange(kv.dirty), [kv.dirty, onDirtyChange])
  React.useEffect(() => () => onDirtyChange(false), [onDirtyChange])

  // Follow server changes (live feed / other tabs) while the editor is clean;
  // while it is dirty, keep the user's edits and flag the conflict instead.
  const serverKey = varsKey(group.vars)
  if (group.id !== loaded.id || (serverKey !== loaded.key && !kv.dirty)) {
    setLoaded({ id: group.id, key: serverKey })
    kv.reset(group.vars)
  }
  const changedElsewhere = kv.dirty && serverKey !== loaded.key && !replace.isPending

  const linked = group.services.length
  const save = async (restart: boolean) => {
    if (!kv.valid || !kv.dirty || replace.isPending) return
    setSaveError(null)
    setPendingAction(restart ? 'restart' : 'save')
    try {
      const saved = await replace.mutateAsync({ vars: kv.vars, restart })
      setLoaded({ id: saved.id, key: varsKey(saved.vars) })
      kv.reset(saved.vars)
      toast.success(`Env group ${saved.name} saved`, {
        description:
          restart && linked > 0
            ? linked === 1
              ? `Restarting ${group.services.join(', ')}.`
              : `Restarting ${linked} services: ${group.services.join(', ')}.`
            : linked > 0
              ? 'Linked services pick up the changes at their next deploy or restart.'
              : undefined,
      })
    } catch (e) {
      setSaveError(errorMessage(e))
    } finally {
      setPendingAction(null)
    }
  }

  const discard = () => {
    setSaveError(null)
    setLoaded({ id: group.id, key: serverKey })
    kv.reset(group.vars)
  }

  return (
    <>
      {changedElsewhere && (
        <Callout
          tone="warning"
          icon={<AlertTriangle />}
          title="This group was changed elsewhere"
          actions={
            <Button size="tiny" onClick={discard}>
              Discard my edits and reload
            </Button>
          }
        >
          Saving now replaces those changes with yours.
        </Callout>
      )}
      <FormCard
        onSubmit={(e) => {
          e.preventDefault()
          void save(false)
        }}
        footer={
          <VariablesFooter
            dirty={kv.dirty}
            valid={kv.valid}
            count={group.vars.length}
            error={saveError}
            canRestart={linked > 0}
            restartHint={`Saves, then restarts: ${group.services.join(', ')}`}
            saving={pendingAction}
            onCancel={discard}
            onSaveAndRestart={() => void save(true)}
          />
        }
      >
        <div className="flex flex-col gap-2 px-5 py-5 md:px-6">
          <KeyValueEditor rows={kv.rows} onChange={kv.setRows} errors={kv.errors} />
          {kv.rows.length === 0 && (
            <p className="text-[13px] text-foreground-lighter">This group has no variables yet.</p>
          )}
        </div>
      </FormCard>
    </>
  )
}

function LinkServiceDialog({
  group,
  services,
  open,
  onOpenChange,
}: {
  group: EnvGroupView
  services: ServiceView[]
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const link = useServiceGroupLink(group.name)
  const [service, setService] = React.useState('')
  const [error, setError] = React.useState<string | null>(null)
  const selectId = React.useId()
  const candidates = services
    .filter((s) => !group.services.includes(s.name))
    .sort((a, b) => a.name.localeCompare(b.name))

  const close = () => {
    onOpenChange(false)
    setService('')
    setError(null)
  }

  return (
    <Dialog open={open} onOpenChange={(o) => (o ? onOpenChange(true) : !link.isPending && close())}>
      <DialogContent size="md">
        <form
          className="flex min-h-0 flex-col"
          onSubmit={(e) => {
            e.preventDefault()
            if (!service) return
            setError(null)
            link.mutate(
              { service, link: true },
              {
                onSuccess: () => {
                  toast.success(`Linked ${group.name} to ${service}`, {
                    description: 'The service receives the variables at its next deploy or restart.',
                  })
                  close()
                },
                onError: (err) => setError(errorMessage(err)),
              },
            )
          }}
        >
          <DialogHeader>
            <DialogTitle>Link a service</DialogTitle>
            <DialogDescription>
              The service gets this group’s variables at its next deploy or restart.
            </DialogDescription>
          </DialogHeader>
          <DialogBody>
            {candidates.length === 0 ? (
              <p className="text-sm text-foreground-light">Every service is already linked to this group.</p>
            ) : (
              <div className="flex flex-col gap-1.5">
                <label htmlFor={selectId} className="text-[13px] font-medium text-foreground">
                  Service
                </label>
                <Select value={service} onValueChange={setService}>
                  <SelectTrigger id={selectId} className="w-full">
                    <SelectValue placeholder="Choose a service" />
                  </SelectTrigger>
                  <SelectContent position="popper">
                    {candidates.map((s) => (
                      <SelectItem key={s.id} value={s.name}>
                        <ServiceTypeIcon type={s.type} className="size-4 text-foreground-lighter" />
                        {s.name}
                        <span className="text-foreground-lighter">· {SERVICE_TYPE_LABELS[s.type]}</span>
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            )}
            {error && (
              <p role="alert" className="text-[13px] text-destructive">
                {error}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button disabled={link.isPending} onClick={close}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={!service} loading={link.isPending}>
              Link service
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function LinkedServicesSection({ group }: { group: EnvGroupView }) {
  const { data: services, isLoading } = useServices()
  const unlink = useServiceGroupLink(group.name)
  const [linkOpen, setLinkOpen] = React.useState(false)
  const [unlinking, setUnlinking] = React.useState<string | null>(null)
  const byName = new Map((services ?? []).map((s) => [s.name, s]))

  return (
    <PageSection
      title="Linked services"
      description="These services receive the group’s variables."
      actions={
        <Button icon={<Link2 />} onClick={() => setLinkOpen(true)} disabled={!services}>
          Link service
        </Button>
      }
    >
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead>Service</TableHead>
            <TableHead className="hidden sm:table-cell">Type</TableHead>
            <TableHead>State</TableHead>
            <TableHead className="w-12">
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {group.services.length === 0 ? (
            <TableMessageRow colSpan={4}>No services are linked to this group yet.</TableMessageRow>
          ) : isLoading ? (
            <TableSkeletonRows columns={4} rows={Math.min(group.services.length, 3)} />
          ) : (
            group.services.map((name) => {
              const s = byName.get(name)
              return (
                <TableRow key={name}>
                  <TableCell>
                    <Link
                      to={`/services/${encodeURIComponent(name)}/environment`}
                      className="inline-flex items-center gap-2 rounded-sm font-medium outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                    >
                      {s && <ServiceTypeIcon type={s.type} className="size-4 text-foreground-lighter" />}
                      {name}
                    </Link>
                  </TableCell>
                  <TableCell className="hidden text-[13px] text-foreground-light sm:table-cell">
                    {s ? SERVICE_TYPE_LABELS[s.type] : '—'}
                  </TableCell>
                  <TableCell>{s ? <ServiceStatePill state={s.state} /> : '—'}</TableCell>
                  <TableCell className="py-1 text-right">
                    <Hint label="Unlink">
                      <Button
                        variant="ghost"
                        size="icon-tiny"
                        icon={<Unlink />}
                        aria-label={`Unlink ${name}`}
                        onClick={() => setUnlinking(name)}
                      />
                    </Hint>
                  </TableCell>
                </TableRow>
              )
            })
          )}
        </TableBody>
      </Table>

      <LinkServiceDialog group={group} services={services ?? []} open={linkOpen} onOpenChange={setLinkOpen} />
      <ConfirmDialog
        open={unlinking !== null}
        onOpenChange={(o) => !o && setUnlinking(null)}
        title={`Unlink ${unlinking ?? ''}?`}
        description={
          <p>
            <span className="font-medium text-foreground">{unlinking}</span> loses the variables of{' '}
            <span className="font-medium text-foreground">{group.name}</span> at its next deploy or restart.
          </p>
        }
        confirmLabel="Unlink"
        variant="warning"
        onConfirm={async () => {
          if (!unlinking) return
          await unlink.mutateAsync({ service: unlinking, link: false })
          toast.success(`Unlinked ${group.name} from ${unlinking}`)
        }}
      />
    </PageSection>
  )
}

function DetailSkeleton() {
  return (
    <div aria-busy="true" aria-label="Loading env group" className="flex flex-col gap-8">
      <div className="flex flex-col gap-2">
        <Skeleton className="h-4 w-24" />
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-4 w-64" />
      </div>
      <Card className="gap-3 p-6">
        {Array.from({ length: 3 }, (_, i) => (
          <div key={i} className="grid grid-cols-[2fr_3fr] gap-2">
            <Skeleton className="h-[34px]" />
            <Skeleton className="h-[34px]" />
          </div>
        ))}
      </Card>
    </div>
  )
}

/** `/env-groups/:name` — variables editor, linked services, delete. */
export function EnvGroupDetailPage() {
  const { name = '' } = useParams()
  const navigate = useNavigate()
  const { data: group, error, refetch, isRefetching } = useEnvGroup(name)
  const [deleteOpen, setDeleteOpen] = React.useState(false)
  const [dirty, setDirty] = React.useState(false)
  const guard = useUnsavedChangesGuard(dirty)

  if (error instanceof ApiError && error.isNotFound && !group) {
    return (
      <PageContainer size="narrow">
        <EmptyState
          size="lg"
          icon={<Braces />}
          title={`Env group “${name}” not found`}
          description="It may have been deleted or renamed."
          actions={
            <Button asChild variant="primary">
              <Link to="/env-groups">All env groups</Link>
            </Button>
          }
        />
      </PageContainer>
    )
  }

  return (
    <PageContainer size="narrow">
      {error && !group ? (
        <>
          <div className="mb-6 text-[13px]">
            <BackLink />
          </div>
          <ErrorState
            error={error}
            title="Could not load the env group"
            onRetry={() => void refetch()}
            retrying={isRefetching}
          />
        </>
      ) : !group ? (
        <DetailSkeleton />
      ) : (
        <>
          <PageHeader
            eyebrow={<BackLink />}
            title={
              <span className="flex min-w-0 items-center gap-3">
                <span
                  aria-hidden="true"
                  className="flex size-11 shrink-0 items-center justify-center rounded-lg border bg-surface-100 text-foreground-light shadow-card"
                >
                  <Braces className="size-5" strokeWidth={1.6} />
                </span>
                <span className="truncate">{group.name}</span>
              </span>
            }
            badges={
              <Badge variant="outline" font="mono" case="normal">
                {plural(group.vars.length, 'variable')}
              </Badge>
            }
            description={
              <span title={dateTime(group.updated_at)}>
                Updated {relativeTime(group.updated_at)} · linked to {plural(group.services.length, 'service')}
              </span>
            }
          />

          <PageSection
            title="Variables"
            description={
              <>
                Shared with every linked service; a service’s own variables take precedence. Values can reference
                datastores and services, e.g.{' '}
                <code className="font-mono text-[12.5px]">{'${{datastore.app-db.connectionString}}'}</code>.
              </>
            }
          >
            <VariablesCard group={group} onDirtyChange={setDirty} />
          </PageSection>

          <LinkedServicesSection group={group} />

          <PageSection title="Danger zone">
            <Card className="border-destructive-border">
              <div className="flex flex-col gap-4 px-5 py-5 sm:flex-row sm:items-center sm:justify-between md:px-6">
                <div className="flex min-w-0 flex-col gap-1">
                  <p className="text-sm font-medium text-foreground">Delete this env group</p>
                  <p className="text-[13px] text-foreground-light">
                    Linked services lose its variables at their next deploy or restart.
                  </p>
                </div>
                <Button variant="danger" icon={<Trash2 />} onClick={() => setDeleteOpen(true)} className="shrink-0">
                  Delete env group
                </Button>
              </div>
            </Card>
          </PageSection>

          <DeleteEnvGroupDialog
            group={group}
            open={deleteOpen}
            onOpenChange={setDeleteOpen}
            onDeleted={() => {
              guard.bypass()
              void navigate('/env-groups', { replace: true })
            }}
          />
        </>
      )}
      {guard.dialog}
    </PageContainer>
  )
}
