import * as React from 'react'
import { useNavigate } from 'react-router'
import { toast } from 'sonner'

import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { LimitControl } from '@/components/patterns/LimitControl'
import { PageSection } from '@/components/patterns/Page'
import { errorMessage } from '@/lib/api/client'
import { useRestartService, useServerInfo, useUpdateService } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'
import {
  cpuFieldOf,
  describeLimitsPatch,
  formatCpus,
  limitsFormFrom,
  limitsPatch,
  limitValue,
  memoryFieldOf,
  memoryHostWarning,
  showLimitError,
  validateLimitsForm,
  type LimitField,
  type LimitsForm,
} from '@/lib/resources'

import { servicePath } from '../../context'
import { useReportDirty, useSyncedForm } from '../hooks'
import { sameForm } from './forms'
import { FormError, SaveFooter, StaleCallout } from './parts'

/** Resources: memory and CPU limits of each instance (and job run). Applied by the next deploy or restart. */
export function ResourcesSection({
  service,
  reportDirty,
}: {
  service: ServiceView
  reportDirty: (id: string, dirty: boolean) => void
}) {
  const name = service.name
  const navigate = useNavigate()
  const update = useUpdateService(name)
  const restart = useRestartService(name)
  const info = useServerInfo().data
  const server = React.useMemo(() => limitsFormFrom(service), [service])
  const form = useSyncedForm<LimitsForm>(server, sameForm)
  const [submitted, setSubmitted] = React.useState(false)
  const [saveError, setSaveError] = React.useState<string | null>(null)
  const ids = React.useId()
  useReportDirty(reportDirty, 'resources', form.dirty)

  const f = form.value
  const errors = validateLimitsForm(f, form.base, info?.docker_cpus)
  const invalid = Object.keys(errors).length > 0
  const memoryError = showLimitError(memoryFieldOf(f), memoryFieldOf(form.base), submitted) ? errors.memory : undefined
  const cpuError = showLimitError(cpuFieldOf(f), cpuFieldOf(form.base), submitted) ? errors.cpu : undefined
  const hostWarning = memoryError ? null : memoryHostWarning(limitValue('memory', memoryFieldOf(f)), info)
  const isCron = service.type === 'cron_job'
  const who = isCron ? 'each run' : 'each instance'

  const setMemory = (v: LimitField) => form.patch({ memory: v.choice, memoryCustom: v.custom })
  const setCpu = (v: LimitField) => form.patch({ cpu: v.choice, cpuCustom: v.custom })

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    setSubmitted(true)
    setSaveError(null)
    if (invalid || !form.dirty) return
    const patch = limitsPatch(form.base, f)
    if (Object.keys(patch).length === 0) {
      form.reset()
      return
    }
    update.mutate(patch, {
      onSuccess: (view) => {
        form.commit(limitsFormFrom(view))
        setSubmitted(false)
        const canRestart = !view.suspended && view.live_deploy_id !== null
        toast.success('Resource limits saved', {
          description: `${describeLimitsPatch(patch)}. ${
            canRestart
              ? `Running ${isCron ? 'jobs keep' : 'instances keep'} the old limits until the next deploy or a restart.`
              : 'They apply on the next deploy.'
          }`,
          action: canRestart
            ? {
                label: 'Restart now',
                onClick: () =>
                  restart.mutate(undefined, {
                    onSuccess: (d) => void navigate(servicePath(name, `/deploys/${d.id}`)),
                    onError: (e2) => toast.error('Could not restart', { description: errorMessage(e2) }),
                  }),
              }
            : undefined,
        })
      },
      onError: (e2) => {
        setSaveError(errorMessage(e2))
        toast.error('Could not save the limits', { description: errorMessage(e2) })
      },
    })
  }

  return (
    <PageSection
      id="resources"
      title="Resources"
      description={`Memory and CPU available to ${who}${isCron ? '' : ' and to the service’s jobs'}. Changes apply on the next deploy or restart.`}
    >
      <FormCard
        aria-label="Resource limits"
        onSubmit={onSubmit}
        footer={
          <SaveFooter
            dirty={form.dirty}
            saving={update.isPending}
            invalid={submitted && invalid}
            onCancel={() => {
              form.reset()
              setSubmitted(false)
              setSaveError(null)
            }}
            hint="Changes apply on the next deploy"
          />
        }
      >
        {form.stale && <StaleCallout onLoad={form.loadLatest} />}
        <FormError message={saveError} />
        <FormRow
          label="Memory limit"
          htmlFor={`${ids}-memory`}
          description={`Most RAM ${who} may use. Past it, the kernel kills the process (out of memory) and Ferry restarts it.`}
          error={memoryError}
        >
          <LimitControl
            kind="memory"
            id={`${ids}-memory`}
            value={memoryFieldOf(f)}
            onChange={setMemory}
            info={info}
            invalid={Boolean(memoryError)}
          />
          {hostWarning && <p className="text-[13px] text-warning">{hostWarning}</p>}
        </FormRow>
        <FormRow
          label="CPU limit"
          htmlFor={`${ids}-cpu`}
          description={
            <>
              CPU time {who} may use, in cores (0.5 = half a core). Past it, the process is slowed down, not stopped.
              {info?.docker_cpus ? ` The Docker host has ${formatCpus(info.docker_cpus)}.` : ''}
            </>
          }
          error={cpuError}
        >
          <LimitControl
            kind="cpu"
            id={`${ids}-cpu`}
            value={cpuFieldOf(f)}
            onChange={setCpu}
            info={info}
            invalid={Boolean(cpuError)}
          />
        </FormRow>
      </FormCard>
    </PageSection>
  )
}
