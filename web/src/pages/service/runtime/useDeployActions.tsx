import * as React from 'react'
import { useNavigate } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { ApiError } from '@/lib/api/client'
import { keys } from '@/lib/api/keys'
import { useCancelDeploy, useRollbackService, useServerInfo } from '@/lib/api/queries'
import type { Deploy, ServiceView } from '@/lib/api/types'
import { DEPLOY_STATUS_LABELS, shortId, shortSha } from '@/lib/format'

import { servicePath } from '../context'

interface Pending {
  type: 'cancel' | 'rollback'
  deploy: Deploy
  open: boolean
}

/**
 * Rollback / cancel with confirmation dialogs, shared by the deploys table and
 * the deploy detail page. Render `dialogs` once; call `requestX(deploy)`.
 * A 409 on cancel ("too late": traffic already switched) closes the dialog
 * with a warning toast instead of an error.
 */
export function useDeployActions(service: ServiceView) {
  // keeps the last deploy while the dialog animates out
  const [pending, setPending] = React.useState<Pending | null>(null)
  const cancel = useCancelDeploy()
  const rollback = useRollbackService(service.name)
  const navigate = useNavigate()
  const qc = useQueryClient()
  const info = useServerInfo()

  const deploy = pending?.deploy
  const close = (o: boolean) => {
    if (!o) setPending((p) => (p ? { ...p, open: false } : p))
  }

  const confirmCancel = async (d: Deploy) => {
    try {
      const res = await cancel.mutateAsync(d.id)
      toast.success(`Deploy ${shortId(d.id)} canceled`, {
        description:
          res.status === 'canceled' ? 'The live version keeps serving traffic.' : DEPLOY_STATUS_LABELS[res.status],
      })
    } catch (e) {
      if (e instanceof ApiError && e.isConflict) {
        toast.warning('Too late to cancel', { description: e.message })
        void qc.invalidateQueries({ queryKey: keys.deploy(d.id) })
        void qc.invalidateQueries({ queryKey: keys.service(service.name) })
        return
      }
      throw e
    }
  }

  const confirmRollback = async (d: Deploy) => {
    const res = await rollback.mutateAsync(d.id)
    const to = servicePath(service.name, `/deploys/${encodeURIComponent(res.id)}`)
    toast.success(`Rolling back to ${shortId(d.id)}`, {
      description: 'A new deploy reuses its image.',
      action: { label: 'View', onClick: () => void navigate(to) },
    })
  }

  const commit = deploy?.commit_sha ? (
    <>
      {' '}
      (commit <span className="font-mono text-foreground">{shortSha(deploy.commit_sha)}</span>
      {deploy.commit_message ? <> “{deploy.commit_message}”</> : null})
    </>
  ) : null

  const dialogs = (
    <>
      <ConfirmDialog
        open={pending?.type === 'cancel' && pending.open}
        onOpenChange={close}
        variant="warning"
        title={deploy ? `Cancel deploy ${shortId(deploy.id)}?` : 'Cancel deploy?'}
        description={
          <>
            <p>
              The build or rollout stops.{' '}
              {service.live_deploy_id
                ? 'The current live version keeps serving traffic.'
                : 'Nothing is live yet, so the service stays undeployed.'}
            </p>
            <p>If traffic has already switched to this deploy it is too late to cancel: roll back instead.</p>
          </>
        }
        confirmLabel="Yes, cancel deploy"
        onConfirm={() => (deploy ? confirmCancel(deploy) : undefined)}
      />
      <ConfirmDialog
        open={pending?.type === 'rollback' && pending.open}
        onOpenChange={close}
        variant="primary"
        title={deploy ? `Roll back to ${shortId(deploy.id)}?` : 'Roll back?'}
        description={
          <>
            <p>
              Ferry starts a new deploy from the image built for this deploy{commit}, with the current environment and
              settings. Nothing is rebuilt.
            </p>
            {service.auto_deploy && service.repo_url && info.data?.github_webhook_enabled && (
              <p>Auto-deploy stays on: the next push to {service.branch} deploys over the rollback.</p>
            )}
          </>
        }
        confirmLabel="Roll back"
        onConfirm={() => (deploy ? confirmRollback(deploy) : undefined)}
      />
    </>
  )

  return {
    requestCancel: (d: Deploy) => setPending({ type: 'cancel', deploy: d, open: true }),
    requestRollback: (d: Deploy) => setPending({ type: 'rollback', deploy: d, open: true }),
    dialogs,
  }
}
