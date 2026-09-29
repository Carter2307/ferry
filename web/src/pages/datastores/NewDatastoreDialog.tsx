import * as React from 'react'
import { useNavigate } from 'react-router'
import { AlertTriangle } from 'lucide-react'
import { RadioGroup as RadioGroupPrimitive } from 'radix-ui'
import { toast } from 'sonner'

import { DatastoreKindIcon } from '@/components/patterns/icons'
import { LimitControl } from '@/components/patterns/LimitControl'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { errorMessage } from '@/lib/api/client'
import { useCreateDatastore, useDatastores, useServerInfo, useServices } from '@/lib/api/queries'
import { DATASTORE_KINDS, type CreateDatastore, type DatastoreKind, type DatastoreView } from '@/lib/api/types'
import { DATASTORE_KIND_LABELS } from '@/lib/format'
import { DEFAULT_LIMIT, limitError, limitValue, type LimitField } from '@/lib/resources'
import { cn } from '@/lib/utils'

import {
  DATASTORE_KIND_DESCRIPTIONS,
  DATASTORE_VERSIONS,
  defaultDatabaseName,
  pgIdentifierError,
  resourceNameError,
} from './lib'

/** Provisioning pulls an image and waits for readiness; past this, finish in the background. */
const FOREGROUND_MS = 1500

type Settled = { ok: true; ds: DatastoreView } | { ok: false; error: unknown }

function Field({
  id,
  label,
  hint,
  error,
  children,
}: {
  id: string
  label: React.ReactNode
  hint?: React.ReactNode
  error?: string | null
  children: React.ReactNode
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-[13px] font-medium text-foreground">
        {label}
      </label>
      {children}
      {error ? (
        <p id={`${id}-error`} role="alert" className="text-[12.5px] text-destructive">
          {error}
        </p>
      ) : (
        hint && (
          <p id={`${id}-hint`} className="text-[12.5px] text-foreground-lighter">
            {hint}
          </p>
        )
      )}
    </div>
  )
}

function notifyProvisioned(result: Settled, toastId?: string | number) {
  if (result.ok && result.ds.status !== 'failed') {
    toast.success(`${result.ds.name} is available`, {
      id: toastId,
      description: `${DATASTORE_KIND_LABELS[result.ds.kind]} ${result.ds.version} is ready for connections.`,
    })
  } else if (result.ok) {
    toast.error(`Provisioning ${result.ds.name} failed`, { id: toastId, description: result.ds.error ?? undefined })
  } else {
    toast.error('Could not create the datastore', { id: toastId, description: errorMessage(result.error) })
  }
}

/** "New datastore" modal: name, kind, version, database / user for Postgres, and optional resource limits. */
export function NewDatastoreDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const navigate = useNavigate()
  const create = useCreateDatastore()
  const { data: datastores } = useDatastores()
  const { data: services } = useServices()
  const { data: info } = useServerInfo()
  const ids = {
    name: React.useId(),
    version: React.useId(),
    database: React.useId(),
    username: React.useId(),
    kind: React.useId(),
    memory: React.useId(),
    cpu: React.useId(),
  }

  const [name, setName] = React.useState('')
  const [kind, setKind] = React.useState<DatastoreKind>('postgres')
  const [version, setVersion] = React.useState(DATASTORE_VERSIONS.postgres.default)
  const [database, setDatabase] = React.useState('')
  const [username, setUsername] = React.useState('')
  const [memory, setMemory] = React.useState<LimitField>(DEFAULT_LIMIT)
  const [cpu, setCpu] = React.useState<LimitField>(DEFAULT_LIMIT)
  /** A custom limit was typed: show its error before submit. */
  const [limitsTouched, setLimitsTouched] = React.useState(false)
  const [touched, setTouched] = React.useState(false)
  const [submitError, setSubmitError] = React.useState<string | null>(null)
  const [pending, setPending] = React.useState(false)

  const reset = () => {
    setName('')
    setKind('postgres')
    setVersion(DATASTORE_VERSIONS.postgres.default)
    setDatabase('')
    setUsername('')
    setMemory(DEFAULT_LIMIT)
    setCpu(DEFAULT_LIMIT)
    setLimitsTouched(false)
    setTouched(false)
    setSubmitError(null)
    setPending(false)
  }

  const taken = React.useMemo(
    () => new Set([...(datastores ?? []).map((d) => d.name), ...(services ?? []).map((s) => s.name)]),
    [datastores, services],
  )
  const trimmed = name.trim()
  const nameError = resourceNameError(trimmed, taken)
  const databaseError = kind === 'postgres' ? pgIdentifierError(database.trim()) : null
  const usernameError = kind === 'postgres' ? pgIdentifierError(username.trim()) : null
  const memoryError = limitError('memory', memory)
  const cpuError = limitError('cpu', cpu, info?.docker_cpus)
  const shownMemoryError = touched || limitsTouched ? memoryError : null
  const shownCpuError = touched || limitsTouched ? cpuError : null
  const valid = !nameError && !databaseError && !usernameError && !memoryError && !cpuError
  const shownNameError = touched || trimmed !== '' ? nameError : null
  const dbPlaceholder = defaultDatabaseName(trimmed) || 'my_db'

  const submit = async () => {
    setTouched(true)
    if (!valid || pending) return
    setSubmitError(null)
    setPending(true)
    const body: CreateDatastore = { name: trimmed, kind }
    if (version) body.version = version
    if (kind === 'postgres') {
      if (database.trim()) body.database = database.trim()
      if (username.trim()) body.username = username.trim()
    }
    const memoryMb = limitValue('memory', memory)
    const cpus = limitValue('cpu', cpu)
    if (memoryMb !== null) body.memory_limit_mb = memoryMb
    if (cpus !== null) body.cpu_limit = cpus
    const settled: Promise<Settled> = create.mutateAsync(body).then(
      (ds) => ({ ok: true as const, ds }),
      (error: unknown) => ({ ok: false as const, error }),
    )
    const first = await Promise.race([
      settled,
      new Promise<null>((resolve) => setTimeout(() => resolve(null), FOREGROUND_MS)),
    ])
    const path = `/datastores/${encodeURIComponent(trimmed)}`
    if (first === null) {
      // Still provisioning (image pull, readiness probe): continue in the background.
      const toastId = toast.loading(`Provisioning ${trimmed}…`, {
        description: `Starting ${DATASTORE_KIND_LABELS[kind]} ${version}. This can take a minute on first use.`,
      })
      void settled.then((r) => notifyProvisioned(r, toastId))
      onOpenChange(false)
      reset()
      void navigate(path)
      return
    }
    if (!first.ok) {
      setPending(false)
      setSubmitError(errorMessage(first.error))
      return
    }
    notifyProvisioned(first)
    onOpenChange(false)
    reset()
    void navigate(path)
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (pending) return
        onOpenChange(o)
        if (!o) reset()
      }}
    >
      <DialogContent size="lg">
        <form
          noValidate
          className="flex min-h-0 flex-col"
          onSubmit={(e) => {
            e.preventDefault()
            void submit()
          }}
        >
          <DialogHeader>
            <DialogTitle>New datastore</DialogTitle>
            <DialogDescription>
              A managed database on the private network. Services reach it by name and get the credentials through env
              references.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="gap-5">
            <Field
              id={ids.name}
              label="Name"
              hint="Also its private hostname. Lowercase letters, digits and “-”."
              error={shownNameError}
            >
              <Input
                id={ids.name}
                mono
                autoFocus
                autoComplete="off"
                spellCheck={false}
                placeholder="my-db"
                value={name}
                maxLength={40}
                onChange={(e) => setName(e.target.value.toLowerCase())}
                aria-invalid={shownNameError ? true : undefined}
                aria-describedby={`${ids.name}-${shownNameError ? 'error' : 'hint'}`}
              />
            </Field>

            <div className="flex flex-col gap-1.5">
              <span id={ids.kind} className="text-[13px] font-medium text-foreground">
                Type
              </span>
              <RadioGroupPrimitive.Root
                aria-labelledby={ids.kind}
                value={kind}
                onValueChange={(v) => {
                  const k = (DATASTORE_KINDS as readonly string[]).includes(v) ? (v as DatastoreKind) : 'postgres'
                  setKind(k)
                  setVersion(DATASTORE_VERSIONS[k].default)
                }}
                className="grid gap-2 sm:grid-cols-2"
              >
                {DATASTORE_KINDS.map((k) => (
                  <RadioGroupPrimitive.Item
                    key={k}
                    value={k}
                    className={cn(
                      'group flex cursor-pointer items-start gap-3 rounded-lg border bg-surface-100 p-3 text-left outline-none transition-colors dark:bg-surface-200',
                      'hover:border-border-stronger focus-visible:ring-2 focus-visible:ring-ring',
                      'data-[state=checked]:border-primary-bright/70 data-[state=checked]:bg-primary-soft',
                    )}
                  >
                    <span className="flex size-8 shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-light group-data-[state=checked]:text-primary">
                      <DatastoreKindIcon kind={k} className="size-4" strokeWidth={1.7} />
                    </span>
                    <span className="flex min-w-0 flex-col gap-0.5">
                      <span className="text-sm font-medium text-foreground">{DATASTORE_KIND_LABELS[k]}</span>
                      <span className="text-[12.5px] leading-snug text-foreground-light">
                        {DATASTORE_KIND_DESCRIPTIONS[k]}
                      </span>
                    </span>
                  </RadioGroupPrimitive.Item>
                ))}
              </RadioGroupPrimitive.Root>
            </div>

            <Field id={ids.version} label="Version" hint={`Runs the official ${kind}:${version}-alpine image.`}>
              {/* Remount per kind: otherwise Radix's hidden native select still lists the previous kind's
                  options for one render, resets to '' and reports an empty value. */}
              <Select
                key={kind}
                value={version}
                onValueChange={(v) => {
                  if (DATASTORE_VERSIONS[kind].options.includes(v)) setVersion(v)
                }}
              >
                <SelectTrigger id={ids.version} className="w-full" aria-describedby={`${ids.version}-hint`}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent position="popper">
                  {DATASTORE_VERSIONS[kind].options.map((v) => (
                    <SelectItem key={v} value={v}>
                      {DATASTORE_KIND_LABELS[kind]} {v}
                      {v === DATASTORE_VERSIONS[kind].default && (
                        <span className="text-foreground-lighter"> (default)</span>
                      )}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>

            {kind === 'postgres' && (
              <div className="grid gap-4 sm:grid-cols-2">
                <Field id={ids.database} label="Database" hint="Optional." error={databaseError}>
                  <Input
                    id={ids.database}
                    mono
                    autoComplete="off"
                    spellCheck={false}
                    placeholder={dbPlaceholder}
                    value={database}
                    maxLength={63}
                    onChange={(e) => setDatabase(e.target.value)}
                    aria-invalid={databaseError ? true : undefined}
                    aria-describedby={`${ids.database}-${databaseError ? 'error' : 'hint'}`}
                  />
                </Field>
                <Field
                  id={ids.username}
                  label="User"
                  hint="Optional. Defaults to the database name."
                  error={usernameError}
                >
                  <Input
                    id={ids.username}
                    mono
                    autoComplete="off"
                    spellCheck={false}
                    placeholder={database.trim() || dbPlaceholder}
                    value={username}
                    maxLength={63}
                    onChange={(e) => setUsername(e.target.value)}
                    aria-invalid={usernameError ? true : undefined}
                    aria-describedby={`${ids.username}-${usernameError ? 'error' : 'hint'}`}
                  />
                </Field>
              </div>
            )}

            <div className="grid gap-4 sm:grid-cols-2">
              <Field
                id={ids.memory}
                label="Memory limit"
                hint="Optional. Change it later without a restart."
                error={shownMemoryError}
              >
                <LimitControl
                  kind="memory"
                  id={ids.memory}
                  value={memory}
                  onChange={(v) => {
                    if (v.custom !== memory.custom) setLimitsTouched(true)
                    setMemory(v)
                  }}
                  info={info}
                  invalid={Boolean(shownMemoryError)}
                  describedBy={`${ids.memory}-${shownMemoryError ? 'error' : 'hint'}`}
                />
              </Field>
              <Field id={ids.cpu} label="CPU limit" hint="Optional. In cores (0.5 = half a core)." error={shownCpuError}>
                <LimitControl
                  kind="cpu"
                  id={ids.cpu}
                  value={cpu}
                  onChange={(v) => {
                    if (v.custom !== cpu.custom) setLimitsTouched(true)
                    setCpu(v)
                  }}
                  info={info}
                  invalid={Boolean(shownCpuError)}
                  describedBy={`${ids.cpu}-${shownCpuError ? 'error' : 'hint'}`}
                />
              </Field>
            </div>

            {submitError && (
              <div
                role="alert"
                className="flex items-start gap-2 rounded-md border border-destructive-border bg-destructive-soft p-3 text-[13px] text-foreground"
              >
                <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
                <span className="break-words">{submitError}</span>
              </div>
            )}
          </DialogBody>
          <DialogFooter>
            <Button
              disabled={pending}
              onClick={() => {
                onOpenChange(false)
                reset()
              }}
            >
              Cancel
            </Button>
            <Button type="submit" variant="primary" loading={pending} disabled={touched && !valid}>
              Create datastore
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
