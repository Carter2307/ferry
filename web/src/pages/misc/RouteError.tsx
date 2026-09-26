import { isRouteErrorResponse, Link, useRouteError } from 'react-router'
import { RefreshCw, TriangleAlert } from 'lucide-react'

import { EmptyState } from '@/components/patterns/EmptyState'
import { PageContainer } from '@/components/patterns/Page'
import { Button } from '@/components/ui/button'
import { errorMessage } from '@/lib/api/client'

/** Error boundary for routes: shows the error message with reload / home actions. */
export function RouteError() {
  const error = useRouteError()
  const message = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : errorMessage(error)
  if (import.meta.env.DEV) console.error(error)
  return (
    <PageContainer size="narrow" className="flex min-h-[60dvh] flex-1 items-center">
      <EmptyState
        size="lg"
        variant="bordered"
        icon={<TriangleAlert className="text-destructive" />}
        title="This page crashed"
        description={
          <span className="flex flex-col gap-2">
            <span>An unexpected error occurred while rendering this page.</span>
            <code className="rounded-md bg-surface-200 px-2 py-1 font-mono text-[12px] break-words text-foreground">{message}</code>
          </span>
        }
        actions={
          <>
            <Button icon={<RefreshCw />} onClick={() => window.location.reload()}>
              Reload
            </Button>
            <Button asChild variant="primary">
              <Link to="/services">Go to services</Link>
            </Button>
          </>
        }
      />
    </PageContainer>
  )
}
