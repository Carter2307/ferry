import { Box, ExternalLink, GitBranch, Upload } from 'lucide-react'

import { ServiceTypeIcon } from '@/components/patterns/icons'
import { ResourceCard } from '@/components/patterns/ResourceCard'
import { ServiceStateLine } from '@/components/patterns/StatusBadge'
import { Badge } from '@/components/ui/badge'
import { isLongRunning, sourceKind, type ServiceView } from '@/lib/api/types'
import { displayUrl, SERVICE_TYPE_LABELS } from '@/lib/format'
import { cn } from '@/lib/utils'
import { servicePath } from '@/pages/service/context'

import { LastDeploy } from './LastDeploy'
import { SERVICE_TYPE_TAGS, sourceLine } from './lib'
import { ServiceMenu } from './ServiceMenu'

const SOURCE_ICONS = { git: GitBranch, image: Box, upload: Upload } as const

/** Where the service can be reached: public URL, cron schedule or private address. */
function Endpoint({ service }: { service: ServiceView }) {
  if (service.url) {
    return (
      <a
        href={service.url}
        target="_blank"
        rel="noreferrer"
        className="relative z-10 inline-flex max-w-full min-w-0 items-center gap-1.5 rounded-sm text-[13px] text-foreground-light outline-none hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring"
      >
        <span className="truncate">{displayUrl(service.url)}</span>
        <ExternalLink className="size-3 shrink-0 opacity-70" aria-hidden="true" />
        <span className="sr-only">(opens in a new tab)</span>
      </a>
    )
  }
  if (service.type === 'cron_job') {
    return (
      <span className="min-w-0 truncate text-[13px] text-foreground-lighter">
        Runs <span className="font-mono text-foreground-light">{service.schedule ?? '—'}</span> UTC
      </span>
    )
  }
  const address = service.internal_port ? `${service.internal_host}:${service.internal_port}` : service.internal_host
  return (
    <span className="min-w-0 truncate text-[13px] text-foreground-lighter">
      Private <span className="font-mono text-foreground-light">{address}</span>
    </span>
  )
}

/** Studio project card: name + ⋮, source line, mono badges, endpoint, status footer. */
export function ServiceCard({ service, now }: { service: ServiceView; now: number }) {
  const kind = sourceKind(service)
  const SourceIcon = SOURCE_ICONS[kind]
  const latest = service.latest_deploy

  return (
    <ResourceCard
      href={servicePath(service.name)}
      name={service.name}
      icon={<ServiceTypeIcon type={service.type} />}
      menu={<ServiceMenu service={service} />}
      subtitle={
        <>
          <SourceIcon className="size-3.5 shrink-0" aria-hidden="true" />
          <span className={cn('truncate', kind !== 'upload' && 'font-mono text-[12.5px]')} title={sourceLine(service)}>
            {kind === 'upload' ? (
              <>
                Upload via <code className="font-mono text-[12.5px] text-foreground-light">ferry up</code>
              </>
            ) : (
              sourceLine(service)
            )}
          </span>
        </>
      }
      badges={
        <>
          <Badge font="mono" shape="square" title={SERVICE_TYPE_LABELS[service.type]}>
            {SERVICE_TYPE_TAGS[service.type]}
          </Badge>
          <Badge font="mono" shape="square" variant="outline" title="Runtime">
            {service.runtime}
          </Badge>
          {isLongRunning(service.type) && (
            <Badge font="mono" shape="square" variant="outline" title={`${service.instances} instance(s)`} case="normal">
              ×{service.instances}
            </Badge>
          )}
          {service.custom_domains.length > 0 && (
            <Badge font="mono" shape="square" variant="outline" title={service.custom_domains.join(', ')}>
              +{service.custom_domains.length} domain{service.custom_domains.length === 1 ? '' : 's'}
            </Badge>
          )}
        </>
      }
      footer={
        <>
          <Endpoint service={service} />
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1">
            <ServiceStateLine state={service.state} />
            {latest && <LastDeploy deploy={latest} now={now} />}
          </div>
        </>
      }
    />
  )
}
