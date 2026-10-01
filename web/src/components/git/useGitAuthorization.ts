import * as React from 'react'
import { useLocation } from 'react-router'
import { toast } from 'sonner'

import { ApiError, errorMessage } from '@/lib/api/client'
import { useAuthorizeGit } from '@/lib/api/queries'
import type { AuthorizeGit, GitConnectionView } from '@/lib/api/types'
import { GIT_PROVIDER_LABELS } from '@/lib/git'
import { clearPending, followAuthorization, gitRedirectUri, savePending } from '@/lib/gitAuthorize'

export interface GitAuthorizationFlow {
  /** `starting`: asking the server where to go; `leaving`: the browser is going to the provider. */
  phase: 'idle' | 'starting' | 'leaving'
  error: string | null
  /** The API's code of `error` (`git_application_required`, …). */
  errorCode: string | null
  /** Start authorizing: the browser leaves for the provider's page and comes back to this one. */
  start: (request: Omit<AuthorizeGit, 'redirect_uri'>) => void
  clearError: () => void
}

/**
 * Authorize a GitHub / GitLab account on the provider's own pages: asks the
 * server where to send the browser and goes there. The provider sends it
 * back to `/git/callback`, which returns to the page this started on.
 * `onConnected` is for the case where nothing is left to do on the provider
 * (an app that is already installed): the account is connected at once.
 */
export function useGitAuthorization(onConnected?: (connection: GitConnectionView) => void): GitAuthorizationFlow {
  const location = useLocation()
  const authorize = useAuthorizeGit()
  const [phase, setPhase] = React.useState<GitAuthorizationFlow['phase']>('idle')
  const [failure, setFailure] = React.useState<{ message: string; code: string | null } | null>(null)

  const start = (request: Omit<AuthorizeGit, 'redirect_uri'>) => {
    if (phase !== 'idle') return
    setFailure(null)
    setPhase('starting')
    authorize.mutate(
      { ...request, redirect_uri: gitRedirectUri() },
      {
        onSuccess: (step) => {
          if (step.status === 'connected' && step.connection) {
            setPhase('idle')
            const c = step.connection
            toast.success(`${GIT_PROVIDER_LABELS[c.provider]} account ${c.account} connected`)
            onConnected?.(c)
            return
          }
          savePending(`${location.pathname}${location.search}`)
          if (!followAuthorization(step)) {
            clearPending()
            setPhase('idle')
            setFailure({ message: 'The server answered without a page to continue on.', code: null })
            return
          }
          setPhase('leaving')
        },
        onError: (e) => {
          setPhase('idle')
          setFailure({ message: errorMessage(e), code: e instanceof ApiError ? e.code : null })
        },
      },
    )
  }

  return {
    phase,
    error: failure?.message ?? null,
    errorCode: failure?.code ?? null,
    start,
    clearError: () => setFailure(null),
  }
}
