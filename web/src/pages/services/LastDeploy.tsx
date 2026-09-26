import { DeployStatusBadge } from '@/components/patterns/StatusBadge'
import { isDeployActive, type Deploy } from '@/lib/api/types'
import { shortSha } from '@/lib/format'
import { cn } from '@/lib/utils'

import { since } from './lib'

/**
 * Latest deploy in one line: a pulsing status pill while it is in flight
 * (queued / building / deploying), else "3m ago · abc1234".
 */
export function LastDeploy({ deploy, now, className }: { deploy: Deploy; now: number; className?: string }) {
  const active = isDeployActive(deploy.status)
  return (
    <span className={cn('flex min-w-0 items-center gap-1.5 text-[12.5px] text-foreground-lighter', className)}>
      <span className="sr-only">Last deploy:</span>
      {active ? (
        <DeployStatusBadge status={deploy.status} className="h-[18px] px-1.5 text-[10px]" />
      ) : (
        <time dateTime={deploy.created_at} title={new Date(deploy.created_at).toLocaleString()}>
          {since(deploy.created_at, now)}
        </time>
      )}
      {deploy.commit_sha && (
        <>
          <span aria-hidden="true">·</span>
          <span className="font-mono text-[12px]" title={deploy.commit_message ?? deploy.commit_sha}>
            {shortSha(deploy.commit_sha)}
          </span>
        </>
      )}
    </span>
  )
}
