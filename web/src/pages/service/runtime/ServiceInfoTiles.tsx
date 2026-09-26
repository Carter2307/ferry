import { Link } from 'react-router'
import {
  Activity,
  Boxes,
  CalendarClock,
  Clock,
  Container,
  GitBranch,
  Globe,
  HardDrive,
  Network,
  Package,
  Rocket,
  Upload,
} from 'lucide-react'

import { CopyButton } from '@/components/patterns/Copy'
import { InfoTile } from '@/components/patterns/InfoTile'
import { DeployStatusBadge, ServiceStatePill } from '@/components/patterns/StatusBadge'
import { isPublicHttp, listensOnPort, type RuntimeStatus, type ServiceView } from '@/lib/api/types'
import {
  dateTime,
  DEPLOY_TRIGGER_LABELS,
  RUNTIME_LABELS,
  relativeTime,
  SERVICE_TYPE_LABELS,
  shortId,
  shortSha,
} from '@/lib/format'

import { servicePath } from '../context'
import { privateAddress, sourceSummary } from './utils'

interface ServiceInfoTilesProps {
  service: ServiceView
  status: RuntimeStatus | undefined
  statusLoading: boolean
}

function statusHint(service: ServiceView, status: RuntimeStatus | undefined): string | undefined {
  const latest = service.latest_deploy
  switch (service.state) {
    case 'live':
      return latest && latest.id === service.live_deploy_id && latest.finished_at
        ? `Live since ${relativeTime(latest.finished_at)}`
        : 'Serving the live deploy'
    case 'deploying':
      return 'A new version is rolling out'
    case 'failed':
      return latest?.error ?? 'The latest deploy failed'
    case 'degraded': {
      if (!status) return 'Some instances are not running'
      const running = status.instances.filter((i) => i.state === 'running').length
      return `${running} of ${status.desired_instances} instances running`
    }
    case 'suspended':
      return isPublicHttp(service.type) ? 'Instances stopped, proxy answers 503' : 'Instances stopped'
    case 'not_deployed':
      return 'Nothing deployed yet'
  }
}

/** Studio "project home" info tiles for one service (2-column grid). */
export function ServiceInfoTiles({ service, status, statusLoading }: ServiceInfoTilesProps) {
  const latest = service.latest_deploy
  const source = sourceSummary(service)
  const address = privateAddress(service)
  const isCron = service.type === 'cron_job'
  const running = status?.instances.filter((i) => i.state === 'running').length ?? 0
  const desired = status?.desired_instances ?? service.instances

  const SourceIcon = source.kind === 'git' ? GitBranch : source.kind === 'image' ? Package : Upload

  return (
    <div className="grid grid-cols-1 gap-x-8 gap-y-6 sm:grid-cols-2">
      <InfoTile
        icon={<Activity />}
        label="Status"
        value={<ServiceStatePill state={service.state} />}
        hint={statusHint(service, status)}
      />

      {isCron ? (
        <InfoTile
          icon={<CalendarClock />}
          label="Schedule"
          value={<span className="font-mono text-[15px]">{service.schedule ?? '—'}</span>}
          hint="Cron expression, UTC"
        />
      ) : (
        <InfoTile
          icon={<Boxes />}
          label="Instances"
          loading={statusLoading && !status}
          value={
            <span className="tabular">
              {service.suspended ? 0 : running}
              <span className="text-foreground-lighter">/{desired}</span>
            </span>
          }
          hint={service.suspended ? 'Suspended' : running === desired ? 'All running' : 'Running / desired'}
        />
      )}

      <InfoTile
        icon={<Network />}
        label="Private address"
        value={
          <span className="flex min-w-0 items-center gap-2">
            <span className="truncate font-mono text-[14px] md:text-[15px]" title={address}>
              {address}
            </span>
            <CopyButton value={address} what="private address" className="shrink-0" />
          </span>
        }
        hint={listensOnPort(service.type) ? 'Reachable by other services' : 'No listening port'}
      />

      <InfoTile
        icon={<SourceIcon />}
        label="Source"
        value={
          <span className="block truncate font-mono text-[14px] md:text-[15px]" title={source.label}>
            {source.label}
          </span>
        }
        hint={
          source.kind === 'upload' ? (
            <span className="font-mono">{source.detail}</span>
          ) : (
            <span title={source.detail}>{source.detail}</span>
          )
        }
      />

      <InfoTile
        icon={<Rocket />}
        label="Latest deploy"
        value={
          latest ? (
            <Link
              to={servicePath(service.name, `/deploys/${encodeURIComponent(latest.id)}`)}
              className="group flex min-w-0 items-center gap-2 rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <DeployStatusBadge status={latest.status} />
              <span className="truncate font-mono text-[14px] text-foreground-light group-hover:text-foreground group-hover:underline md:text-[15px]">
                {latest.commit_sha ? shortSha(latest.commit_sha) : shortId(latest.id)}
              </span>
            </Link>
          ) : (
            <span className="text-foreground-light">No deploys yet</span>
          )
        }
        hint={
          latest ? (
            <span title={latest.commit_message ?? undefined}>
              {latest.commit_message ? `${latest.commit_message} · ` : `${DEPLOY_TRIGGER_LABELS[latest.trigger]} · `}
              {relativeTime(latest.created_at)}
            </span>
          ) : undefined
        }
      />

      <InfoTile
        icon={<Container />}
        label="Runtime"
        value={RUNTIME_LABELS[service.runtime]}
        hint={`${SERVICE_TYPE_LABELS[service.type]}${service.port ? ` · port ${service.port}` : ''}`}
      />

      {isPublicHttp(service.type) ? (
        <InfoTile
          icon={<Globe />}
          label="Domains"
          value={
            service.hosts[0] ? (
              <span className="block truncate font-mono text-[14px] md:text-[15px]" title={service.hosts.join('\n')}>
                {service.hosts[0]}
              </span>
            ) : (
              '—'
            )
          }
          hint={
            service.hosts.length > 1
              ? `+${service.hosts.length - 1} more · ${service.custom_domains.length} custom`
              : service.custom_domains.length > 0
                ? `${service.custom_domains.length} custom`
                : 'Default host only'
          }
        />
      ) : (
        <InfoTile
          icon={<HardDrive />}
          label="Disk"
          value={
            service.disk_mount_path ? (
              <span className="font-mono text-[14px] md:text-[15px]">{service.disk_mount_path}</span>
            ) : (
              <span className="text-foreground-light">No disk</span>
            )
          }
          hint={service.disk_mount_path ? 'Persistent volume' : 'Filesystem is ephemeral'}
        />
      )}

      <InfoTile
        icon={<Clock />}
        label="Created"
        value={
          <time dateTime={service.created_at} title={dateTime(service.created_at)}>
            {relativeTime(service.created_at)}
          </time>
        }
        hint={dateTime(service.created_at)}
      />
    </div>
  )
}
