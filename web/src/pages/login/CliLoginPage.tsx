import * as React from 'react'
import { Link, Navigate, useSearchParams } from 'react-router'
import { CircleCheck, CircleSlash, Terminal } from 'lucide-react'

import { FerryLogo } from '@/components/patterns/icons'
import { Button } from '@/components/ui/button'
import { ApiError, errorMessage } from '@/lib/api/client'
import { useAnswerCliLogin, useCliLogin } from '@/lib/api/queries'
import type { CliLoginView } from '@/lib/api/types'
import { useAuthRedirect } from '@/lib/useAuthRedirect'

/**
 * `/cli-login?id=…` — where `ferry login` sends the browser: the signed-in
 * administrator checks that the code is the one the terminal shows, and
 * approves. The terminal then gets an API token of its own. Shown without
 * the app shell: it is one decision, then back to the terminal.
 */
export function CliLoginPage() {
  const [params] = useSearchParams()
  const id = params.get('id')?.trim() ?? ''
  // Signing in (or setting the server up) comes back here.
  const redirect = useAuthRedirect()
  if (redirect) return <Navigate to={redirect} replace />
  return (
    <div className="flex min-h-dvh flex-col items-center justify-center gap-6 bg-background px-6 py-10">
      <FerryLogo className="size-8" />
      <div className="w-full max-w-[440px] rounded-lg border bg-surface-100 p-6 shadow-card sm:p-8">
        {id ? <Request id={id} /> : <Gone />}
      </div>
      <Link
        to="/services"
        className="rounded-sm text-[13px] text-foreground-light underline-offset-4 outline-none hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring"
      >
        Open the dashboard
      </Link>
    </div>
  )
}

function Message({
  icon,
  title,
  children,
}: {
  icon: React.ReactNode
  title: string
  children: React.ReactNode
}) {
  return (
    <div className="flex flex-col items-center gap-3 text-center">
      <span aria-hidden="true" className="[&_svg]:size-6">
        {icon}
      </span>
      <h1 className="text-lg font-medium text-foreground">{title}</h1>
      <p className="text-sm leading-relaxed text-foreground-light">{children}</p>
    </div>
  )
}

function Gone() {
  return (
    <Message icon={<CircleSlash className="text-foreground-lighter" />} title="This login request is gone">
      It expired, or it was already answered. Run <code className="font-mono text-foreground">ferry login</code> again
      in your terminal.
    </Message>
  )
}

function Request({ id }: { id: string }) {
  const request = useCliLogin(id)
  const answer = useAnswerCliLogin(id)

  if (request.isPending) {
    return (
      <p role="status" className="text-center text-sm text-foreground-light">
        Loading the request…
      </p>
    )
  }
  if (request.isError || !request.data) {
    if (request.error instanceof ApiError && request.error.isNotFound) return <Gone />
    return (
      <Message icon={<CircleSlash className="text-destructive" />} title="Could not load the request">
        {errorMessage(request.error)}
      </Message>
    )
  }
  const view: CliLoginView = request.data
  if (view.status === 'approved') {
    return (
      <Message icon={<CircleCheck className="text-success" />} title="Terminal connected">
        Go back to your terminal: <span className="font-medium text-foreground">{view.name}</span> now has an API token
        of its own. You can revoke it at any time under{' '}
        <Link
          to="/server?section=account"
          className="rounded-sm text-primary underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
        >
          Server → Account
        </Link>
        .
      </Message>
    )
  }
  if (view.status === 'denied') {
    return (
      <Message icon={<CircleSlash className="text-foreground-lighter" />} title="Request denied">
        The terminal was told, and got nothing.
      </Message>
    )
  }

  const gone = answer.error instanceof ApiError && answer.error.isNotFound
  if (gone) return <Gone />
  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col items-center gap-3 text-center">
        <Terminal className="size-6 text-foreground-lighter" aria-hidden="true" />
        <h1 className="text-lg font-medium text-foreground">Connect a terminal?</h1>
        <p className="text-sm leading-relaxed text-foreground-light">
          <span className="font-medium text-foreground">{view.name}</span> asks to manage this server from the command
          line.
        </p>
      </div>
      <div className="flex flex-col items-center gap-2 rounded-md border bg-surface-200 px-4 py-5">
        <span className="text-[13px] text-foreground-light">The terminal shows this code</span>
        <span className="font-mono text-[26px] leading-none font-medium tracking-[0.12em] text-foreground">
          {view.code}
        </span>
      </div>
      <p className="text-[13px] leading-relaxed text-foreground-light">
        Approve only if you started <code className="font-mono text-foreground">ferry login</code> yourself and your
        terminal shows the same code. Approving gives that terminal full access to this server.
      </p>
      {answer.isError && !gone && (
        <p role="alert" className="text-[13px] text-destructive">
          {errorMessage(answer.error)}
        </p>
      )}
      <div className="flex gap-3">
        <Button size="lg" className="flex-1" disabled={answer.isPending} onClick={() => answer.mutate(false)}>
          Deny
        </Button>
        <Button
          size="lg"
          variant="primary"
          className="flex-1"
          loading={answer.isPending && answer.variables === true}
          disabled={answer.isPending}
          onClick={() => answer.mutate(true)}
        >
          Approve
        </Button>
      </div>
    </div>
  )
}
