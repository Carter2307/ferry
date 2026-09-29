import * as React from 'react'
import { useLocation, useNavigate } from 'react-router'

import { MonoLabel } from '@/components/patterns/MonoLabel'
import { isPublicHttp, type ServiceView } from '@/lib/api/types'
import { cn } from '@/lib/utils'

import { useDirtyRegistry } from './config/hooks'
import { BuildDeploySection } from './config/settings/BuildDeploySection'
import { CronSection } from './config/settings/CronSection'
import { DangerZoneSection } from './config/settings/DangerZoneSection'
import { DeployHookSection } from './config/settings/DeployHookSection'
import { DomainsSection } from './config/settings/DomainsSection'
import { GeneralSection } from './config/settings/GeneralSection'
import { ResourcesSection } from './config/settings/ResourcesSection'
import { ScalingSection } from './config/settings/ScalingSection'
import { UnsavedChangesGuard } from '@/components/patterns/UnsavedChangesGuard'
import { useServiceOutlet } from './context'

/** `/services/:name/settings` — Studio settings layout: stacked section cards + an "on this page" index. */
export function SettingsPage() {
  const { service } = useServiceOutlet()
  return <SettingsView key={service.id} service={service} />
}

function SettingsView({ service }: { service: ServiceView }) {
  const navigate = useNavigate()
  const { hash } = useLocation()
  const dirty = useDirtyRegistry()
  const [deleted, setDeleted] = React.useState(false)

  // Deep links such as `/settings#resources` (from the OOM notice on the overview).
  React.useEffect(() => {
    if (hash) document.getElementById(decodeURIComponent(hash.slice(1)))?.scrollIntoView({ block: 'start' })
  }, [hash])

  React.useEffect(() => {
    if (deleted) void navigate('/services', { replace: true })
  }, [deleted, navigate])

  const isCron = service.type === 'cron_job'
  const publicHttp = isPublicHttp(service.type)
  const sections = React.useMemo(
    () => [
      { id: 'general', label: 'General' },
      { id: 'build', label: 'Build & deploy' },
      isCron ? { id: 'schedule', label: 'Cron schedule' } : { id: 'scaling', label: 'Scaling' },
      { id: 'resources', label: 'Resources' },
      ...(publicHttp ? [{ id: 'domains', label: 'Custom domains' }] : []),
      { id: 'deploy-hook', label: 'Deploy hook' },
      { id: 'danger', label: 'Danger zone' },
    ],
    [isCron, publicHttp],
  )

  return (
    <>
      <UnsavedChangesGuard when={dirty.any && !deleted} />
      <div className="grid grid-cols-1 gap-10 2xl:grid-cols-[minmax(0,880px)_200px] 2xl:justify-between">
        <div className="flex w-full max-w-[880px] min-w-0 flex-col">
          <GeneralSection service={service} />
          <BuildDeploySection service={service} reportDirty={dirty.set} />
          {isCron ? (
            <CronSection service={service} reportDirty={dirty.set} />
          ) : (
            <ScalingSection service={service} reportDirty={dirty.set} />
          )}
          <ResourcesSection service={service} reportDirty={dirty.set} />
          {publicHttp && <DomainsSection service={service} />}
          <DeployHookSection service={service} />
          <DangerZoneSection service={service} onDeleted={() => setDeleted(true)} />
        </div>
        <OnThisPage sections={sections} />
      </div>
    </>
  )
}

/** Sticky section index (very wide screens only). */
function OnThisPage({ sections }: { sections: { id: string; label: string }[] }) {
  const [active, setActive] = React.useState<string | null>(null)

  React.useEffect(() => {
    const targets = sections.map((s) => document.getElementById(s.id)).filter((el): el is HTMLElement => el !== null)
    if (targets.length === 0 || typeof IntersectionObserver === 'undefined') return
    const visible = new Map<string, boolean>()
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) visible.set(e.target.id, e.isIntersecting)
        const first = sections.find((s) => visible.get(s.id))
        if (first) setActive(first.id)
      },
      { rootMargin: '0px 0px -55% 0px', threshold: 0 },
    )
    for (const t of targets) io.observe(t)
    return () => io.disconnect()
  }, [sections])

  return (
    <nav aria-label="On this page" className="hidden 2xl:block">
      <div className="sticky top-6 flex flex-col gap-2">
        <MonoLabel as="h2" className="px-2.5">
          On this page
        </MonoLabel>
        <ul className="flex flex-col">
          {sections.map((s) => (
            <li key={s.id}>
              <button
                type="button"
                onClick={() => {
                  document.getElementById(s.id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
                  setActive(s.id)
                }}
                aria-current={active === s.id ? 'location' : undefined}
                className={cn(
                  'flex h-[30px] w-full cursor-pointer items-center rounded-md px-2.5 text-left text-[13px] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring',
                  active === s.id
                    ? 'bg-selection font-medium text-foreground'
                    : 'text-foreground-light hover:bg-surface-200 hover:text-foreground',
                  s.id === 'danger' && active !== s.id && 'text-destructive/90 hover:text-destructive',
                )}
              >
                {s.label}
              </button>
            </li>
          ))}
        </ul>
      </div>
    </nav>
  )
}
