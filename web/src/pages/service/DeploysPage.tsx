import * as React from 'react'
import { ChevronDown } from 'lucide-react'

import { PageSection } from '@/components/patterns/Page'
import { StatusDot } from '@/components/patterns/StatusBadge'
import { Button } from '@/components/ui/button'
import { useLive } from '@/lib/api/events'
import { useDeploys } from '@/lib/api/queries'
import type { Deploy } from '@/lib/api/types'

import { useServiceOutlet, type ServiceOutletContext } from './context'
import { DeploysTable } from './runtime/DeploysTable'
import { useDeployActions } from './runtime/useDeployActions'

const PAGE = 25
/** Server cap (`LimitQuery`). */
const MAX = 500

/** `/services/:name/deploys` — deploy history, live through the change feed. */
export function DeploysPage() {
  const { service, name } = useServiceOutlet()
  // fresh local state (page size, kept rows) per service
  return <Deploys key={service.id} service={service} name={name} />
}

function Deploys({ service, name }: ServiceOutletContext) {
  const [limit, setLimit] = React.useState(PAGE)
  const deploys = useDeploys(name, limit)
  const actions = useDeployActions(service)
  const liveMode = useLive((s) => s.mode)

  // Keep showing the previous page while a bigger one loads (no skeleton flash).
  const [shown, setShown] = React.useState<Deploy[] | undefined>(undefined)
  if (deploys.data && deploys.data !== shown) setShown(deploys.data)
  const rows = deploys.data ?? shown

  const canLoadMore = rows !== undefined && rows.length >= limit && limit < MAX

  return (
    <PageSection
      title="Deploys"
      description="Every build and rollout of this service, newest first."
      actions={
        <span className="inline-flex items-center gap-2 text-[12px] text-foreground-lighter" aria-live="polite">
          <StatusDot tone={liveMode === 'live' ? 'success' : 'neutral'} pulse={liveMode === 'live'} />
          {liveMode === 'live' ? 'Live updates' : liveMode === 'polling' ? 'Refreshing every 3s' : 'Connecting…'}
        </span>
      }
    >
      <DeploysTable
        service={service}
        deploys={rows}
        loading={deploys.isLoading}
        error={deploys.error}
        onCancel={actions.requestCancel}
        onRollback={actions.requestRollback}
      />
      {canLoadMore && (
        <div className="flex justify-center">
          <Button
            icon={<ChevronDown />}
            loading={deploys.isFetching && deploys.data === undefined}
            onClick={() => setLimit((l) => Math.min(MAX, l + PAGE))}
          >
            Load older deploys
          </Button>
        </div>
      )}
      {actions.dialogs}
    </PageSection>
  )
}
