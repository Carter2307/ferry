import * as React from 'react'
import { toast } from 'sonner'

import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { LimitControl } from '@/components/patterns/LimitControl'
import { PageSection } from '@/components/patterns/Page'
import { errorMessage } from '@/lib/api/client'
import { useServerInfo, useUpdateDatastore } from '@/lib/api/queries'
import type { DatastoreView } from '@/lib/api/types'
import {
  cpuFieldOf,
  describeLimitsPatch,
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
import { useSyncedForm } from '@/pages/service/config/hooks'
import { sameForm } from '@/pages/service/config/settings/forms'
import { FormError, SaveFooter, StaleCallout } from '@/pages/service/config/settings/parts'

/** Datastore memory / CPU limits: `PATCH /api/v1/datastores/{id}`, applied to the running container in place. */
export function DatastoreResourcesSection({ ds }: { ds: DatastoreView }) {
  const update = useUpdateDatastore(ds.name)
  const info = useServerInfo().data
  const server = React.useMemo(() => limitsFormFrom(ds), [ds])
  const form = useSyncedForm<LimitsForm>(server, sameForm)
  const [submitted, setSubmitted] = React.useState(false)
  const [saveError, setSaveError] = React.useState<string | null>(null)
  const ids = React.useId()

  const f = form.value
  const errors = validateLimitsForm(f, form.base, info?.docker_cpus)
  const invalid = Object.keys(errors).length > 0
  const memoryError = showLimitError(memoryFieldOf(f), memoryFieldOf(form.base), submitted) ? errors.memory : undefined
  const cpuError = showLimitError(cpuFieldOf(f), cpuFieldOf(form.base), submitted) ? errors.cpu : undefined
  const hostWarning = memoryError ? null : memoryHostWarning(limitValue('memory', memoryFieldOf(f)), info)

  const setMemory = (v: LimitField) => form.patch({ memory: v.choice, memoryCustom: v.custom })
  const setCpu = (v: LimitField) => form.patch({ cpu: v.choice, cpuCustom: v.custom })

  const save = (patch: ReturnType<typeof limitsPatch>) => {
    setSaveError(null)
    update.mutate(patch, {
      onSuccess: (view) => {
        form.commit(limitsFormFrom(view))
        setSubmitted(false)
        toast.success('Resource limits applied', {
          description: `${describeLimitsPatch(patch)}. ${
            view.status === 'available'
              ? 'The running container was updated in place, without a restart.'
              : view.status === 'creating'
                ? 'Applied to its container, or it is created with them.'
                : 'Applied to its container: provisioning retries with these limits.'
          }`,
        })
      },
      onError: (e2) => {
        setSaveError(errorMessage(e2))
        // The limits may be saved while applying them failed: re-sending them retries.
        toast.error('Could not change the limits', {
          description: errorMessage(e2),
          action: { label: 'Retry', onClick: () => save(patch) },
        })
      },
    })
  }

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
    save(patch)
  }

  return (
    <PageSection
      id="resources"
      title="Resources"
      description="Memory and CPU available to the container. Changes apply right away, without a restart."
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
            hint="Applied right away, no restart"
            saveLabel="Apply limits"
          />
        }
      >
        {form.stale && <StaleCallout onLoad={form.loadLatest} />}
        <FormError message={saveError} />
        <FormRow
          label="Memory limit"
          htmlFor={`${ids}-memory`}
          description={
            ds.kind === 'redis'
              ? 'Redis keeps every key in memory: past the limit, the kernel kills it (out of memory). Keep it above the data size.'
              : 'Most RAM Postgres may use. Past it, the kernel kills it (out of memory); lowering it below the current use can do the same.'
          }
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
          description="CPU time the container may use, in cores (0.5 = half a core). Past it, queries slow down."
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
