import * as React from 'react'

import { PageSection } from '@/components/patterns/Page'
import { useServiceStatus } from '@/lib/api/queries'

import { useServiceOutlet, type ServiceOutletContext } from './context'
import { AvailabilityCard } from './runtime/AvailabilityCard'
import { InstancesSection } from './runtime/InstancesSection'
import { RunJobDialog } from './config/jobs/RunJobDialog'
import { ScaleDialog } from './runtime/ScaleDialog'
import { ServiceInfoTiles } from './runtime/ServiceInfoTiles'
import { ServiceTopology } from './runtime/ServiceTopology'

/** Instance metrics refresh period (only while the tab is visible). */
const STATUS_REFRESH_MS = 4000

/**
 * `/services/:name` — Studio "project home": info tiles next to a dotted-grid
 * canvas of the service's connections, live instance metrics, and the
 * availability card with the runtime actions.
 */
export function OverviewPage() {
  const { service, name } = useServiceOutlet()
  return <Overview key={service.id} service={service} name={name} />
}

function Overview({ service, name }: ServiceOutletContext) {
  const longRunning = service.type !== 'cron_job'
  const status = useServiceStatus(name, { enabled: longRunning && !service.suspended, intervalMs: STATUS_REFRESH_MS })
  const [scaleOpen, setScaleOpen] = React.useState(false)
  const [runJobOpen, setRunJobOpen] = React.useState(false)

  return (
    <>
      <section aria-label="Service summary" className="mb-10 @container">
        <div className="grid gap-8 @min-[1000px]:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)] @min-[1000px]:gap-10">
          <ServiceInfoTiles service={service} status={status.data} statusLoading={status.isLoading} />
          <ServiceTopology service={service} />
        </div>
      </section>

      <InstancesSection
        service={service}
        status={status.data}
        updatedAt={status.dataUpdatedAt}
        loading={status.isLoading}
        error={status.error}
        onRetry={() => void status.refetch()}
        retrying={status.isRefetching}
        onScale={() => setScaleOpen(true)}
        refreshSeconds={STATUS_REFRESH_MS / 1000}
      />

      <PageSection title="Service availability" description="Deploy, restart, scale or pause this service.">
        <AvailabilityCard service={service} onScale={() => setScaleOpen(true)} onRunJob={() => setRunJobOpen(true)} />
      </PageSection>

      <ScaleDialog service={service} open={scaleOpen} onOpenChange={setScaleOpen} />
      <RunJobDialog service={service} open={runJobOpen} onOpenChange={setRunJobOpen} />
    </>
  )
}
