import * as React from 'react'

import { FerryLogo } from '@/components/patterns/icons'
import { Button } from '@/components/ui/button'
import { loadAuthStatus } from '@/lib/api/session'
import { useAuth } from '@/stores/auth'

/** Shown instead of the app when the server couldn't say whether this browser is signed in. */
export function ServerUnreachable() {
  const error = useAuth((s) => s.error)
  const [retrying, setRetrying] = React.useState(false)
  const retry = async () => {
    setRetrying(true)
    await loadAuthStatus()
    setRetrying(false)
  }
  return (
    <div className="flex h-dvh items-center justify-center bg-background px-6">
      <div className="flex max-w-sm flex-col items-center gap-4 text-center" role="alert">
        <FerryLogo className="size-8" />
        <div className="flex flex-col gap-1">
          <h1 className="text-base font-medium text-foreground">Can’t reach the Ferry server</h1>
          <p className="text-[13px] leading-relaxed text-foreground-light">
            {error ?? 'The server did not answer.'} Check that <code className="font-mono">ferryd</code> is running (
            <code className="font-mono">ferryd status</code>), then try again.
          </p>
        </div>
        <Button variant="primary" loading={retrying} onClick={() => void retry()}>
          Try again
        </Button>
      </div>
    </div>
  )
}
