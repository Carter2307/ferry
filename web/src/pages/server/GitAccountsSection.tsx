import * as React from 'react'
import { Link } from 'react-router'
import { AlertTriangle, KeyRound, Unplug } from 'lucide-react'
import { toast } from 'sonner'

import { ConnectGitDialog } from '@/components/git/ConnectGitDialog'
import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { Callout, EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { GitProviderIcon } from '@/components/patterns/icons'
import { PageSection } from '@/components/patterns/Page'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { useDisconnectGit, useGitConnections } from '@/lib/api/queries'
import { GIT_PROVIDERS, type GitConnectionView, type GitProvider } from '@/lib/api/types'
import { plural, relativeTime } from '@/lib/format'
import { connectionHost, GIT_PROVIDER_LABELS, scopeWarning, tokenExpiry } from '@/lib/git'
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

/** The services cloned through a connection, as links. */
function UsedBy({ services }: { services: string[] }) {
  if (services.length === 0) return <>Not used by any service</>
  return (
    <>
      Used by{' '}
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

function AccountRow({
  connection,
  onReplaceToken,
  onDisconnect,
}: {
  connection: GitConnectionView
  onReplaceToken: () => void
  onDisconnect: () => void
}) {
  const label = GIT_PROVIDER_LABELS[connection.provider]
  const host = connectionHost(connection)
  const expiry = tokenExpiry(connection.token_expires_at)
  const warning = scopeWarning(connection)
  return (
    <li className="flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center md:px-6">
      <span
        aria-hidden="true"
        className="flex size-9 shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-light dark:bg-surface-200"
      >
        <GitProviderIcon provider={connection.provider} className="size-[18px]" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <p className="flex min-w-0 flex-wrap items-baseline gap-x-2 text-sm">
          <span className="truncate font-medium text-foreground">{connection.account}</span>
          <span className="truncate text-[13px] text-foreground-lighter">
            {[connection.account_name, host ?? label].filter(Boolean).join(' · ')}
          </span>
        </p>
        <p className="text-[12.5px] text-foreground-lighter">
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
        </p>
        <p className="text-[12.5px] text-foreground-lighter">
          <UsedBy services={connection.services} />
        </p>
        {warning && <p className="text-[12.5px] text-warning">{warning}</p>}
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <Button size="tiny" icon={<KeyRound />} onClick={onReplaceToken}>
          Replace token
        </Button>
        <Button size="tiny" variant="danger" icon={<Unplug />} onClick={onDisconnect}>
          Disconnect
        </Button>
      </div>
    </li>
  )
}

/**
 * Server → Connections → Git accounts: the connected GitHub / GitLab
 * accounts, with their token's state and the services cloned through them.
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
      description="GitHub and GitLab accounts connected with an access token: pick their repositories when you create a service, private ones included."
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
                onReplaceToken={() => setDialog({ connection: c })}
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
        title={`Disconnect ${leaving?.account ?? ''}?`}
        description={
          <p>
            Ferry deletes its copy of the token. The token itself stays valid until you revoke it on{' '}
            {leaving ? GIT_PROVIDER_LABELS[leaving.provider] : 'the provider'}.
          </p>
        }
        confirmLabel="Disconnect"
        onConfirm={() => {
          const c = leaving
          if (!c) return
          return disconnect.mutateAsync({ id: c.id, force: c.services.length > 0 }).then(() => {
            toast.success(`${c.account} disconnected`)
          })
        }}
      >
        {leaving && leaving.services.length > 0 && (
          <Callout tone="warning" icon={<AlertTriangle />}>
            {plural(leaving.services.length, 'service')} ({leaving.services.join(', ')}){' '}
            {leaving.services.length === 1 ? 'clones' : 'clone'} with this account. They keep their repository, but
            their next deploys clone without credentials, which fails for private repositories.
          </Callout>
        )}
      </ConfirmDialog>
    </PageSection>
  )
}
