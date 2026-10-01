import * as React from 'react'
import { Link } from 'react-router'
import { AlertTriangle, ExternalLink, KeyRound, RefreshCw, Unplug } from 'lucide-react'
import { toast } from 'sonner'

import { ConnectGitDialog } from '@/components/git/ConnectGitDialog'
import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { Callout, EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { GitProviderIcon } from '@/components/patterns/icons'
import { PageSection } from '@/components/patterns/Page'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { useDisconnectGit, useGitConnections } from '@/lib/api/queries'
import { GIT_PROVIDERS, type GitConnectionView, type GitProvider } from '@/lib/api/types'
import { plural, relativeTime } from '@/lib/format'
import { accountName, connectionHost, GIT_AUTH_LABELS, GIT_PROVIDER_LABELS, scopeWarning, tokenExpiry } from '@/lib/git'
import { cn } from '@/lib/utils'

type DialogState = { provider?: GitProvider; connection?: GitConnectionView } | null

function ConnectButtons({ onConnect, size }: { onConnect: (provider: GitProvider) => void; size: 'sm' | 'md' }) {
  return (
    <>
      {GIT_PROVIDERS.map((p) => (
        <Button key={p} size={size} icon={<GitProviderIcon provider={p} />} onClick={() => onConnect(p)}>
          Connect {GIT_PROVIDER_LABELS[p]}
        </Button>
      ))}
    </>
  )
}

/** The services whose repository a connection clones, as links. */
function UsedBy({ services }: { services: string[] }) {
  if (services.length === 0) return <>Clones for no service yet</>
  return (
    <>
      Clones for{' '}
      {services.map((name, i) => (
        <React.Fragment key={name}>
          {i > 0 && ', '}
          <Link
            to={`/services/${encodeURIComponent(name)}/settings`}
            className="rounded-sm font-mono text-[12.5px] text-foreground-light underline-offset-4 outline-none hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring"
          >
            {name}
          </Link>
        </React.Fragment>
      ))}
    </>
  )
}

/** How the account is authorized, in one line. */
function Authorization({ connection }: { connection: GitConnectionView }) {
  const expiry = tokenExpiry(connection.token_expires_at)
  const saved = <span title={connection.updated_at}>authorized {relativeTime(connection.updated_at)}</span>
  if (connection.auth === 'github_app') {
    return (
      <>
        {GIT_AUTH_LABELS.github_app}{' '}
        {connection.app_url ? (
          <a
            href={connection.app_url}
            target="_blank"
            rel="noreferrer"
            className="rounded-sm font-mono text-foreground-light underline-offset-4 outline-none hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring"
          >
            {connection.app_slug}
          </a>
        ) : (
          <span className="font-mono">{connection.app_slug}</span>
        )}
        {connection.status === 'connected' && (
          <>
            {' · '}
            {connection.repository_selection === 'all' ? 'all repositories' : 'selected repositories'}
            {' · '}
            {saved}
          </>
        )}
      </>
    )
  }
  if (connection.auth === 'oauth') {
    return (
      <>
        {GIT_AUTH_LABELS.oauth}
        {connection.client_id && (
          <>
            {' '}
            <span className="font-mono">…{connection.client_id.slice(-6)}</span>
          </>
        )}
        {connection.status === 'connected' && (
          <>
            {' · '}
            {saved}
          </>
        )}
      </>
    )
  }
  return (
    <>
      Token <span className="font-mono">{connection.token_hint}</span>
      {expiry && (
        <>
          {' · '}
          <span className={cn(expiry.tone === 'expired' && 'text-destructive', expiry.tone === 'soon' && 'text-warning')}>
            {expiry.label}
          </span>
        </>
      )}
      {' · '}
      <span title={connection.updated_at}>saved {relativeTime(connection.updated_at)}</span>
    </>
  )
}

function AccountRow({
  connection,
  onAuthorize,
  onDisconnect,
}: {
  connection: GitConnectionView
  onAuthorize: () => void
  onDisconnect: () => void
}) {
  const label = GIT_PROVIDER_LABELS[connection.provider]
  const host = connectionHost(connection)
  const warning = scopeWarning(connection)
  const pending = connection.status === 'pending'
  return (
    <li className="flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center md:px-6">
      <span
        aria-hidden="true"
        className="flex size-9 shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-light dark:bg-surface-200"
      >
        <GitProviderIcon provider={connection.provider} className="size-[18px]" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm">
          <span className="truncate font-medium text-foreground">{accountName(connection)}</span>
          <span className="truncate text-[13px] text-foreground-lighter">
            {[connection.account_name, host ?? label].filter(Boolean).join(' · ')}
          </span>
          {pending && (
            <Badge variant="warning" case="normal">
              Not finished
            </Badge>
          )}
        </p>
        <p className="text-[12.5px] text-foreground-lighter">
          <Authorization connection={connection} />
        </p>
        <p className="text-[12.5px] text-foreground-lighter">
          {pending ? (
            connection.auth === 'github_app' ? (
              'The app is registered but not installed on an account yet.'
            ) : (
              'The application is saved but no account authorized it yet.'
            )
          ) : (
            <UsedBy services={connection.services} />
          )}
        </p>
        {warning && <p className="text-[12.5px] text-warning">{warning}</p>}
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        {pending ? (
          <Button size="tiny" variant="primary" onClick={onAuthorize}>
            Finish connecting
          </Button>
        ) : connection.auth === 'token' ? (
          <Button size="tiny" icon={<KeyRound />} onClick={onAuthorize}>
            Replace token
          </Button>
        ) : connection.auth === 'github_app' && connection.manage_url ? (
          <Button asChild size="tiny" iconRight={<ExternalLink />}>
            <a href={connection.manage_url} target="_blank" rel="noreferrer">
              Repositories
            </a>
          </Button>
        ) : (
          <Button size="tiny" icon={<RefreshCw />} onClick={onAuthorize}>
            Authorize again
          </Button>
        )}
        <Button size="tiny" variant="danger" icon={<Unplug />} onClick={onDisconnect}>
          Disconnect
        </Button>
      </div>
    </li>
  )
}

/** What stays behind on the provider when a connection is removed here. */
function leftBehind(connection: GitConnectionView): string {
  const label = GIT_PROVIDER_LABELS[connection.provider]
  if (connection.auth === 'github_app') {
    return `Ferry forgets the app’s key. The GitHub App ${connection.app_slug ?? ''} stays on GitHub until you delete it in its settings there.`
  }
  if (connection.auth === 'oauth') {
    return `Ferry forgets its tokens and the application. The authorization stays on ${label} until you revoke it there.`
  }
  return `Ferry deletes its copy of the token. The token itself stays valid until you revoke it on ${label}.`
}

/**
 * Server → Connections → Git accounts: the GitHub / GitLab accounts this
 * server is authorized to read, how each is authorized, and the services
 * whose repository each one clones. They belong to the server: no service
 * chooses one.
 */
export function GitAccountsSection() {
  const connections = useGitConnections()
  const disconnect = useDisconnectGit()
  const [dialog, setDialog] = React.useState<DialogState>(null)
  const [leaving, setLeaving] = React.useState<GitConnectionView | null>(null)
  const list = connections.data ?? []

  return (
    <PageSection
      id="git"
      title="Git accounts"
      description="GitHub and GitLab accounts this server is authorized to read: pick their repositories when you create a service, private ones included. Every service whose repository is on an account’s host is cloned with it."
      actions={list.length > 0 ? <ConnectButtons size="sm" onConnect={(provider) => setDialog({ provider })} /> : undefined}
    >
      {connections.isPending ? (
        <Skeleton className="h-[86px] w-full" />
      ) : connections.isError && !connections.data ? (
        <ErrorState
          error={connections.error}
          title="Could not load the git accounts"
          onRetry={() => void connections.refetch()}
          retrying={connections.isRefetching}
        />
      ) : list.length === 0 ? (
        <EmptyState
          title="No git account connected"
          description="Connect one to choose a repository from a list instead of pasting its URL, and to deploy private repositories."
          actions={<ConnectButtons size="md" onConnect={(provider) => setDialog({ provider })} />}
        />
      ) : (
        <Card>
          <ul className="divide-y" aria-label="Connected git accounts">
            {list.map((c) => (
              <AccountRow
                key={c.id}
                connection={c}
                onAuthorize={() => setDialog({ connection: c })}
                onDisconnect={() => setLeaving(c)}
              />
            ))}
          </ul>
        </Card>
      )}

      <ConnectGitDialog
        open={dialog !== null}
        onOpenChange={(o) => !o && setDialog(null)}
        provider={dialog?.provider}
        connection={dialog?.connection}
      />

      <ConfirmDialog
        open={leaving !== null}
        onOpenChange={(o) => !o && setLeaving(null)}
        title={`Disconnect ${leaving ? accountName(leaving) : ''}?`}
        description={<p>{leaving ? leftBehind(leaving) : ''}</p>}
        confirmLabel="Disconnect"
        onConfirm={() => {
          const c = leaving
          if (!c) return
          return disconnect.mutateAsync({ id: c.id, force: c.services.length > 0 }).then(() => {
            toast.success(`${accountName(c)} disconnected`)
          })
        }}
      >
        {leaving && leaving.services.length > 0 && (
          <Callout tone="warning" icon={<AlertTriangle />}>
            The repository of {plural(leaving.services.length, 'service')} ({leaving.services.join(', ')}){' '}
            {leaving.services.length === 1 ? 'is' : 'are'} cloned with this account. They keep their repository, but
            their next deploys clone without credentials, which fails for private repositories.
          </Callout>
        )}
      </ConfirmDialog>
    </PageSection>
  )
}
