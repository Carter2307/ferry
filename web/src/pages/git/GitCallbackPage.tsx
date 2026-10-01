import * as React from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, Navigate, useLocation, useNavigate } from 'react-router'
import { CircleAlert } from 'lucide-react'
import { toast } from 'sonner'

import { FerryLogo } from '@/components/patterns/icons'
import { Button } from '@/components/ui/button'
import { errorMessage } from '@/lib/api/client'
import { endpoints } from '@/lib/api/endpoints'
import { keys } from '@/lib/api/keys'
import { GIT_PROVIDER_LABELS } from '@/lib/git'
import {
  callbackFromQuery,
  clearPending,
  followAuthorization,
  gitConnectedState,
  hasCallback,
  readPending,
  safeReturnTo,
} from '@/lib/gitAuthorize'
import { useAuthRedirect } from '@/lib/useAuthRedirect'

/**
 * `/git/callback` — where GitHub / GitLab send the browser back while an
 * account is being authorized. Hands the query parameters to the server and
 * follows its answer: the provider's next page, or back to where the user
 * was. Shown without the app shell: it is only passed through.
 */
export function GitCallbackPage() {
  const location = useLocation()
  // Signing in comes back here with the same parameters.
  const redirect = useAuthRedirect()
  if (redirect) return <Navigate to={redirect} replace />
  return <GitCallback search={location.search} />
}

function GitCallback({ search }: { search: string }) {
  const qc = useQueryClient()
  const navigate = useNavigate()
  const [failure, setFailure] = React.useState<string | null>(null)
  const [returnTo] = React.useState(() => safeReturnTo(readPending()?.returnTo))
  // What the provider sent works once: never hand it over twice.
  const handed = React.useRef(false)

  React.useEffect(() => {
    if (handed.current) return
    handed.current = true
    const callback = callbackFromQuery(search)
    const fail = (message: string) => {
      clearPending()
      setFailure(message)
    }
    if (!hasCallback(callback)) {
      fail('Nothing to finish: this page is where GitHub and GitLab send you back while an account is being connected.')
      return
    }
    endpoints
      .gitCallback(callback)
      .then((step) => {
        // GitHub: the app is registered, its installation page comes next.
        if (step.status === 'redirect') {
          if (!followAuthorization(step)) fail('The server answered without a page to continue on.')
          return
        }
        const connection = step.connection
        if (!connection) {
          fail('The server answered without the connection.')
          return
        }
        clearPending()
        void qc.invalidateQueries({ queryKey: keys.git() })
        toast.success(`${GIT_PROVIDER_LABELS[connection.provider]} account ${connection.account} connected`)
        void navigate(returnTo, { replace: true, state: gitConnectedState(connection.id) })
      })
      .catch((e: unknown) => fail(errorMessage(e)))
  }, [search, qc, navigate, returnTo])

  return (
    <div className="flex min-h-dvh items-center justify-center bg-background px-6 py-10">
      <main className="flex w-full max-w-[420px] flex-col items-center gap-5 text-center">
        <FerryLogo className={failure === null ? 'size-9 animate-pulse' : 'size-9'} />
        {failure === null ? (
          <div role="status" className="flex flex-col gap-1.5">
            <h1 className="text-lg font-medium text-foreground">Connecting the account…</h1>
            <p className="text-sm text-foreground-light">Ferry is finishing the authorization with the provider.</p>
          </div>
        ) : (
          <>
            <div role="alert" className="flex flex-col items-center gap-1.5">
              <CircleAlert className="size-5 text-destructive" aria-hidden="true" />
              <h1 className="text-lg font-medium text-foreground">The account was not connected</h1>
              <p className="text-sm break-words text-foreground-light">{failure}</p>
            </div>
            <Button asChild variant="primary">
              <Link to={returnTo} replace>
                Back to Ferry
              </Link>
            </Button>
          </>
        )}
      </main>
    </div>
  )
}
