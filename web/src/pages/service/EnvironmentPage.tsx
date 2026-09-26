import * as React from 'react'
import { Link, useNavigate } from 'react-router'
import { toast } from 'sonner'

import { newRowId, useKeyValueRows } from '@/components/patterns/kv-rows'
import { PageSection } from '@/components/patterns/Page'
import { errorMessage } from '@/lib/api/client'
import { useDatastores, useEnvGroups, useReplaceServiceEnv, useServiceEnv, useServices } from '@/lib/api/queries'
import type { EnvVar, ServiceView } from '@/lib/api/types'

import { EffectiveEnvTable } from './config/env/EffectiveEnvTable'
import type { KnownTargets } from './config/env/effective'
import { LinkedGroupsCard } from './config/env/LinkedGroupsCard'
import { ReferenceHelp } from './config/env/ReferenceHelp'
import { ServiceVariablesCard } from './config/env/ServiceVariablesCard'
import { UnsavedChangesGuard } from '@/components/patterns/UnsavedChangesGuard'
import { servicePath, useServiceOutlet } from './context'

function sameVars(a: readonly EnvVar[], b: readonly EnvVar[]): boolean {
  return a.length === b.length && a.every((v, i) => v.key === b[i]?.key && v.value === b[i]?.value)
}

/** `/services/:name/environment` — own variables, linked groups, merged view, reference help. */
export function EnvironmentPage() {
  const { service } = useServiceOutlet()
  // A different service is a different form (fresh state, no leaked edits).
  return <EnvironmentView key={service.id} service={service} />
}

function EnvironmentView({ service }: { service: ServiceView }) {
  const name = service.name
  const navigate = useNavigate()
  const env = useServiceEnv(name)
  const groups = useEnvGroups()
  const datastores = useDatastores()
  const services = useServices()
  const replace = useReplaceServiceEnv(name)
  const kv = useKeyValueRows(undefined)
  const [revealed, setRevealed] = React.useState(false)
  const [saving, setSaving] = React.useState<'save' | 'restart' | null>(null)

  // Server snapshot handling (render-time, no effects):
  // - first load / not dirty → load the server list into the editor;
  // - dirty → keep the user's edits and flag that the server changed.
  const [seen, setSeen] = React.useState<EnvVar[] | undefined>(undefined)
  const [base, setBase] = React.useState<EnvVar[]>([])
  const [remote, setRemote] = React.useState<EnvVar[] | null>(null)
  const data = env.data
  if (data && data !== seen) {
    setSeen(data)
    if (seen === undefined || !kv.dirty) {
      kv.reset(data)
      setBase(data)
      setRemote(null)
    } else if (!sameVars(data, base)) {
      setRemote(data)
    }
  }

  const loadServer = (vars: EnvVar[]) => {
    kv.reset(vars)
    setBase(vars)
    setRemote(null)
  }

  const canRestart = Boolean(service.live_deploy_id) && !service.suspended

  const save = (restart: boolean) => {
    if (!kv.dirty || !kv.valid || saving) return
    setSaving(restart ? 'restart' : 'save')
    replace.mutate(
      { vars: kv.vars, restart },
      {
        onSuccess: (vars) => {
          loadServer(vars)
          if (restart) {
            toast.success(`Saved — restarting ${name}`, {
              description: 'The new environment is live once the restart deploy finishes.',
              action: { label: 'View deploys', onClick: () => void navigate(servicePath(name, '/deploys')) },
            })
          } else {
            toast.success('Environment saved', {
              description: canRestart ? 'It applies on the next deploy or restart.' : undefined,
            })
          }
        },
        onError: (e) => toast.error('Could not save the environment', { description: errorMessage(e) }),
        onSettled: () => setSaving(null),
      },
    )
  }

  const addVariable = (key: string, value: string) => {
    kv.setRows([...kv.rows.filter((r) => r.key.trim() !== '' || r.value !== ''), { id: newRowId(), key, value }])
    toast.success(`Added ${key}`, { description: 'Save to apply it.' })
    document.getElementById('service-variables')?.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
  }

  const known = React.useMemo<KnownTargets | null>(
    () =>
      datastores.data && services.data
        ? { datastores: datastores.data.map((d) => d.name), services: services.data.map((s) => s.name) }
        : null,
    [datastores.data, services.data],
  )
  const envReady = Boolean(data)

  return (
    <>
      <UnsavedChangesGuard when={kv.dirty} />
      <div className="grid grid-cols-1 gap-x-8 gap-y-10 2xl:grid-cols-[minmax(0,1fr)_320px]">
        <div className="flex min-w-0 flex-col">
          <PageSection
            title="Environment variables"
            description={
              <>
                Available to builds (as secrets, never baked into the image) and to running containers. Share variables
                across services with{' '}
                <Link to="/env-groups" className="text-primary underline-offset-4 hover:underline">
                  environment groups
                </Link>
                .
              </>
            }
          >
            <ServiceVariablesCard
              serviceName={name}
              rows={kv.rows}
              onRowsChange={kv.setRows}
              errors={kv.errors}
              vars={kv.vars}
              dirty={kv.dirty}
              valid={kv.valid}
              // isPending, not isLoading: a paused retry (offline / hidden tab) must not look like an empty list
              loading={env.isPending && !env.error}
              loadError={data ? null : env.error}
              onRetry={() => void env.refetch()}
              retrying={env.isRefetching}
              remoteChanged={remote !== null}
              onLoadRemote={() => remote && loadServer(remote)}
              canRestart={canRestart}
              saving={saving}
              onSave={save}
              onCancel={() => kv.reset(base)}
              revealed={revealed}
              onToggleReveal={() => setRevealed((r) => !r)}
            />
            <LinkedGroupsCard
              service={service}
              groups={groups.data}
              loading={groups.isPending}
              error={groups.error}
              onRetry={() => void groups.refetch()}
            />
          </PageSection>

          <EffectiveEnvTable
            service={service}
            ownVars={kv.vars}
            dirty={kv.dirty}
            groups={groups.data}
            loading={env.isPending || (groups.isPending && service.env_groups.length > 0)}
            error={data ? null : env.error}
            known={known}
          />
        </div>

        <aside aria-label="Reference help" className="min-w-0 2xl:sticky 2xl:top-6 2xl:self-start">
          <ReferenceHelp
            service={service}
            datastores={datastores.data}
            services={services.data}
            loading={datastores.isPending || services.isPending}
            ownVars={kv.vars}
            onAdd={envReady && !saving ? addVariable : null}
          />
        </aside>
      </div>
    </>
  )
}
