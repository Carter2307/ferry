import * as React from 'react'
import { toast } from 'sonner'

import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { PageSection } from '@/components/patterns/Page'
import { Input } from '@/components/ui/input'
import { errorMessage } from '@/lib/api/client'
import { useUpdateService } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'
import { plural } from '@/lib/format'

import { useReportDirty, useSyncedForm } from '../hooks'
import {
  MAX_INSTANCES,
  sameForm,
  scalingFormFrom,
  scalingPatch,
  supportsDisk,
  validateScalingForm,
  type ScalingForm,
} from './forms'
import { FormError, SaveFooter, StaleCallout } from './parts'

/** Scaling: instance count (applied at once) and the persistent disk mount path. */
export function ScalingSection({
  service,
  reportDirty,
}: {
  service: ServiceView
  reportDirty: (id: string, dirty: boolean) => void
}) {
  const name = service.name
  const update = useUpdateService(name)
  const server = React.useMemo(() => scalingFormFrom(service), [service])
  const form = useSyncedForm<ScalingForm>(server, sameForm)
  const [submitted, setSubmitted] = React.useState(false)
  const [saveError, setSaveError] = React.useState<string | null>(null)
  useReportDirty(reportDirty, 'scaling', form.dirty)

  const f = form.value
  const disk = supportsDisk(service.type)
  const errors = validateScalingForm(f, service.type)
  const invalid = Object.keys(errors).length > 0
  const err = (k: keyof ScalingForm) => (submitted || f[k] !== form.base[k] ? errors[k] : undefined)

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    setSubmitted(true)
    setSaveError(null)
    if (invalid || !form.dirty) return
    const patch = scalingPatch(form.base, f, service.type)
    if (Object.keys(patch).length === 0) {
      form.reset()
      return
    }
    update.mutate(patch, {
      onSuccess: (view) => {
        form.commit(scalingFormFrom(view))
        setSubmitted(false)
        const parts: string[] = []
        if (patch.instances !== undefined) parts.push(`Scaled to ${plural(view.instances, 'instance')}`)
        if (patch.disk_mount_path !== undefined) {
          parts.push(view.disk_mount_path ? `Disk at ${view.disk_mount_path} (next deploy)` : 'Disk removed (next deploy)')
        }
        toast.success(parts.join(' · ') || 'Scaling saved')
      },
      onError: (e2) => {
        setSaveError(errorMessage(e2))
        toast.error('Could not save scaling', { description: errorMessage(e2) })
      },
    })
  }

  return (
    <PageSection id="scaling" title="Scaling" description="How many copies run, and where persistent data lives.">
      <FormCard
        aria-label="Scaling settings"
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
            hint="Instances change right away"
          />
        }
      >
        {form.stale && <StaleCallout onLoad={form.loadLatest} />}
        <FormError message={saveError} />
        <FormRow
          label="Instances"
          htmlFor="svc-instances"
          description={
            <>
              Identical containers behind the proxy (1–{MAX_INSTANCES}). Applied immediately, without a new deploy
              {service.suspended ? ' — takes effect when the service is resumed.' : '.'}
            </>
          }
          error={err('instances')}
        >
          <Input
            id="svc-instances"
            type="number"
            min={1}
            max={MAX_INSTANCES}
            inputMode="numeric"
            mono
            className="sm:max-w-32"
            value={f.instances}
            aria-invalid={err('instances') ? true : undefined}
            onChange={(e) => form.patch({ instances: e.target.value })}
          />
        </FormRow>
        {disk && (
          <FormRow
            label="Disk mount path"
            htmlFor="svc-disk"
            description="Mount a persistent volume at this path. Services with a disk run 1 instance and stop the old container before starting the new one (brief downtime). Empty = no disk."
            error={err('disk_mount_path')}
          >
            <Input
              id="svc-disk"
              mono
              placeholder="/var/data"
              autoComplete="off"
              spellCheck={false}
              value={f.disk_mount_path}
              aria-invalid={err('disk_mount_path') ? true : undefined}
              onChange={(e) => form.patch({ disk_mount_path: e.target.value })}
            />
          </FormRow>
        )}
      </FormCard>
    </PageSection>
  )
}
