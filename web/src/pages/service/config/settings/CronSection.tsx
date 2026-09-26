import * as React from 'react'
import { toast } from 'sonner'

import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { PageSection } from '@/components/patterns/Page'
import { Input } from '@/components/ui/input'
import { errorMessage } from '@/lib/api/client'
import { useUpdateService } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'
import { cn } from '@/lib/utils'

import { useReportDirty, useSyncedForm } from '../hooks'
import { describeSchedule, sameForm, scheduleError } from './forms'
import { FormError, SaveFooter, StaleCallout } from './parts'

const PRESETS: { expr: string; label: string }[] = [
  { expr: '*/15 * * * *', label: 'Every 15 min' },
  { expr: '@hourly', label: 'Hourly' },
  { expr: '0 3 * * *', label: 'Daily 03:00' },
  { expr: '0 9 * * 1-5', label: 'Weekdays 09:00' },
  { expr: '@weekly', label: 'Weekly' },
]

interface CronForm {
  schedule: string
}

/** Cron schedule (cron jobs only), evaluated in UTC. */
export function CronSection({
  service,
  reportDirty,
}: {
  service: ServiceView
  reportDirty: (id: string, dirty: boolean) => void
}) {
  const update = useUpdateService(service.name)
  const server = React.useMemo<CronForm>(() => ({ schedule: service.schedule ?? '' }), [service.schedule])
  const form = useSyncedForm<CronForm>(server, sameForm)
  const [saveError, setSaveError] = React.useState<string | null>(null)
  useReportDirty(reportDirty, 'cron', form.dirty)

  const value = form.value.schedule
  const error = form.dirty ? scheduleError(value) : null
  const description = describeSchedule(value)

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    setSaveError(null)
    if (!form.dirty || scheduleError(value)) return
    const schedule = value.trim()
    update.mutate(
      { schedule },
      {
        onSuccess: (view) => {
          form.commit({ schedule: view.schedule ?? '' })
          toast.success('Schedule saved', { description: describeSchedule(view.schedule ?? '') ?? view.schedule ?? undefined })
        },
        onError: (e2) => {
          setSaveError(errorMessage(e2))
          toast.error('Could not save the schedule', { description: errorMessage(e2) })
        },
      },
    )
  }

  return (
    <PageSection id="schedule" title="Cron schedule" description="When the job runs. Runs missed while the server is down are skipped.">
      <FormCard
        aria-label="Cron schedule"
        onSubmit={onSubmit}
        footer={
          <SaveFooter
            dirty={form.dirty}
            saving={update.isPending}
            invalid={Boolean(error)}
            onCancel={() => {
              form.reset()
              setSaveError(null)
            }}
            hint="Takes effect at the next tick"
          />
        }
      >
        {form.stale && <StaleCallout onLoad={form.loadLatest} />}
        <FormError message={saveError} />
        <FormRow
          label="Schedule"
          htmlFor="svc-schedule"
          description={
            <>
              Standard 5-field cron expression (minute hour day-of-month month day-of-week) or an alias such as{' '}
              <code className="font-mono">@daily</code>. Evaluated in UTC.
            </>
          }
          error={error}
        >
          <Input
            id="svc-schedule"
            mono
            placeholder="0 3 * * *"
            autoComplete="off"
            spellCheck={false}
            value={value}
            aria-invalid={error ? true : undefined}
            aria-describedby="svc-schedule-human"
            onChange={(e) => form.patch({ schedule: e.target.value })}
          />
          <p id="svc-schedule-human" className="text-[13px] text-foreground-light" aria-live="polite">
            {description ?? (value.trim() && !scheduleError(value) ? 'Custom schedule' : ' ')}
          </p>
          <div className="flex flex-wrap gap-1.5" role="group" aria-label="Common schedules">
            {PRESETS.map((p) => (
              <button
                key={p.expr}
                type="button"
                onClick={() => form.patch({ schedule: p.expr })}
                aria-pressed={value.trim() === p.expr}
                className={cn(
                  'inline-flex h-6 cursor-pointer items-center rounded-full border px-2.5 text-[12px] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring',
                  value.trim() === p.expr
                    ? 'border-primary/40 bg-primary-soft text-primary'
                    : 'border-border-strong text-foreground-light hover:border-border-stronger hover:bg-surface-200 hover:text-foreground',
                )}
                title={p.expr}
              >
                {p.label}
              </button>
            ))}
          </div>
        </FormRow>
      </FormCard>
    </PageSection>
  )
}
