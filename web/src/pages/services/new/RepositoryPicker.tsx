import * as React from 'react'
import { Link } from 'react-router'
import { AlertTriangle, Archive, BookMarked, Check, ExternalLink, Lock, Plus, RefreshCw, Search } from 'lucide-react'

import { ConnectGitDialog } from '@/components/git/ConnectGitDialog'
import { Callout, EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { GitProviderIcon } from '@/components/patterns/icons'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Hint } from '@/components/ui/tooltip'
import { ApiError, errorMessage } from '@/lib/api/client'
import { useGitConnections, useGitRepositories } from '@/lib/api/queries'
import type { GitConnectionView, GitProvider, GitRepository } from '@/lib/api/types'
import { relativeTime } from '@/lib/format'
import { connectedAccounts, connectionLabel, filterRepositories, GIT_PROVIDER_LABELS, scopeWarning } from '@/lib/git'
import { cn } from '@/lib/utils'

import type { PickedRepository } from './form'

/** Rows rendered at once; the search narrows longer lists down. */
const MAX_ROWS = 100

interface RepositoryPickerProps {
  /** Id of the focusable element (for the label and for error focus). */
  id: string
  value: PickedRepository | null
  /** The account to show first (the one that was just connected), by connection id. */
  account?: string | null
  /** `repository` is the provider's record of a newly picked one (for its default branch and name). */
  onChange: (value: PickedRepository | null, repository?: GitRepository) => void
  invalid?: boolean
  describedBy?: string
}

type DialogState = { provider?: GitProvider; connection?: GitConnectionView } | null

function ConnectButtons({ onConnect }: { onConnect: (provider: GitProvider) => void }) {
  return (
    <>
      {(['github', 'gitlab'] as const).map((p) => (
        <Button key={p} size="md" icon={<GitProviderIcon provider={p} />} onClick={() => onConnect(p)}>
          Connect {GIT_PROVIDER_LABELS[p]}
        </Button>
      ))}
    </>
  )
}

function RepositoryRow({ repository, selected, onPick }: { repository: GitRepository; selected: boolean; onPick: () => void }) {
  const empty = repository.default_branch === null
  return (
    <li role="presentation">
      <button
        type="button"
        role="option"
        aria-selected={selected}
        disabled={empty}
        onClick={onPick}
        className={cn(
          'flex w-full cursor-pointer items-center gap-2.5 px-3 py-2 text-left outline-none transition-colors',
          'hover:bg-surface-200 focus-visible:bg-surface-200 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset',
          'disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:bg-transparent',
          selected && 'bg-selection',
        )}
      >
        {repository.private ? (
          <Lock className="size-3.5 shrink-0 text-foreground-lighter" aria-hidden="true" />
        ) : (
          <BookMarked className="size-3.5 shrink-0 text-foreground-lighter" aria-hidden="true" />
        )}
        <span className="min-w-0 flex-1 truncate text-[13px]">
          <span className="text-foreground-lighter">{repository.owner}/</span>
          <span className="font-medium text-foreground">{repository.name}</span>
        </span>
        {repository.private && (
          <Badge variant="outline" case="normal">
            Private
          </Badge>
        )}
        {repository.archived && (
          <Badge variant="outline" case="normal">
            <Archive aria-hidden="true" /> Archived
          </Badge>
        )}
        {empty ? (
          <span className="shrink-0 text-[12px] text-foreground-lighter">No commits yet</span>
        ) : (
          <span className="hidden shrink-0 text-[12px] text-foreground-lighter tabular-nums sm:inline">
            {relativeTime(repository.updated_at)}
          </span>
        )}
        <Check className={cn('size-3.5 shrink-0 text-primary', !selected && 'invisible')} aria-hidden="true" />
      </button>
    </li>
  )
}

/** What to do when the provider no longer accepts how an account was authorized. */
function reconnectLabel(connection: GitConnectionView): string {
  if (connection.auth === 'token') return 'Replace the token'
  if (connection.auth === 'oauth') return 'Authorize again'
  return 'Connect again'
}

/** The repositories of one connection: search + list. */
function RepositoryList({
  id,
  connection,
  value,
  onPick,
  onReconnect,
  invalid,
  describedBy,
}: {
  id: string
  connection: GitConnectionView
  value: PickedRepository | null
  onPick: (repository: GitRepository) => void
  onReconnect: () => void
  invalid?: boolean
  describedBy?: string
}) {
  const repositories = useGitRepositories(connection.id)
  const [query, setQuery] = React.useState('')
  const label = GIT_PROVIDER_LABELS[connection.provider]
  const all = repositories.data?.repositories
  const matches = React.useMemo(() => filterRepositories(all ?? [], query), [all, query])
  const warning = scopeWarning(connection)
  // A GitHub App reads what the account selected for it, on GitHub.
  const selection = connection.auth === 'github_app' && connection.manage_url ? connection.manage_url : null

  if (repositories.isPending) {
    return (
      <div className="flex flex-col gap-2" aria-label="Loading repositories" aria-busy="true">
        <Skeleton className="h-[34px] w-full" />
        <div className="flex flex-col gap-px rounded-md border p-1.5">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-8 w-full" />
          ))}
        </div>
      </div>
    )
  }
  if (repositories.isError && !all) {
    const rejected = repositories.error instanceof ApiError && repositories.error.code === 'git_authorization_rejected'
    return rejected ? (
      <Callout
        tone="destructive"
        icon={<AlertTriangle />}
        title={`${label} no longer accepts this authorization`}
        actions={
          <Button size="tiny" onClick={onReconnect}>
            {reconnectLabel(connection)}
          </Button>
        }
      >
        <span className="break-words">{errorMessage(repositories.error)}</span>
      </Callout>
    ) : (
      <ErrorState
        error={repositories.error}
        title={`Could not list the repositories of ${connection.account}`}
        onRetry={() => void repositories.refetch()}
        retrying={repositories.isRefetching}
      />
    )
  }

  const shown = matches.slice(0, MAX_ROWS)
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <div className="relative min-w-0 flex-1">
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-foreground-lighter" aria-hidden="true" />
          <Input
            id={id}
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={`Search ${all?.length ?? 0} repositories…`}
            aria-label="Search repositories"
            aria-invalid={invalid || undefined}
            aria-describedby={describedBy}
            autoComplete="off"
            spellCheck={false}
            className="pl-8"
            onKeyDown={(e) => {
              // Enter picks nothing by itself, and must not submit the form.
              if (e.key === 'Enter') e.preventDefault()
            }}
          />
        </div>
        <Hint label="Refresh the list">
          <Button
            size="icon-md"
            aria-label="Refresh the list of repositories"
            icon={<RefreshCw className={cn(repositories.isRefetching && 'animate-spin')} />}
            onClick={() => void repositories.refetch()}
          />
        </Hint>
      </div>

      {shown.length > 0 ? (
        <ul
          role="listbox"
          aria-label={`Repositories of ${connection.account}`}
          className="max-h-[296px] divide-y overflow-y-auto rounded-md border bg-surface-100 dark:bg-surface-200/40"
        >
          {shown.map((r) => (
            <RepositoryRow
              key={r.id}
              repository={r}
              selected={value?.connectionId === connection.id && value.cloneUrl === r.clone_url}
              onPick={() => onPick(r)}
            />
          ))}
        </ul>
      ) : (
        <p className="rounded-md border border-dashed border-border-stronger px-3 py-6 text-center text-[13px] text-foreground-light">
          {all?.length === 0
            ? selection
              ? `The app can’t read any repository of ${connection.account} yet.`
              : `${connection.account} has no repository Ferry can read.`
            : `No repository matches “${query.trim()}”.`}
        </p>
      )}

      {matches.length > shown.length && (
        <p className="text-[12.5px] text-foreground-lighter">
          Showing {shown.length} of {matches.length} repositories. Search to narrow the list down.
        </p>
      )}
      {repositories.data?.truncated && (
        <p className="text-[12.5px] text-foreground-lighter">
          This account can access more repositories than {label} lists here: the 1000 most recently updated are shown.
          For another one, give its URL instead.
        </p>
      )}
      {selection && (
        <p className="text-[12.5px] text-foreground-lighter">
          {connection.repository_selection === 'all'
            ? 'The GitHub App reads every repository of this account.'
            : 'The GitHub App reads the repositories you selected for it.'}{' '}
          <a
            href={selection}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-1 rounded-sm text-primary underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
          >
            Change them on GitHub
            <ExternalLink className="size-3" aria-hidden="true" />
          </a>
          <span className="sr-only"> (opens in a new tab)</span>, then refresh the list.
        </p>
      )}
      {warning && <p className="text-[12.5px] text-warning">{warning}</p>}
      {repositories.isError && (
        <p role="alert" className="text-[12.5px] text-destructive">
          Refreshing failed: {errorMessage(repositories.error)}
        </p>
      )}
    </div>
  )
}

/**
 * Pick the repository to deploy from a GitHub / GitLab account: connect an
 * account (on the provider's own pages), choose the account, search its
 * repositories, click one.
 */
export function RepositoryPicker({ id, value, account: preferred, onChange, invalid, describedBy }: RepositoryPickerProps) {
  const connections = useGitConnections()
  const [dialog, setDialog] = React.useState<DialogState>(null)
  const [accountId, setAccountId] = React.useState<string | null>(value?.connectionId ?? preferred ?? null)
  const [choosing, setChoosing] = React.useState(value === null)

  // Only accounts whose authorization is finished have repositories to list.
  const list = connectedAccounts(connections.data)
  const unfinished = (connections.data ?? []).filter((c) => c.status === 'pending')
  const account = list.find((c) => c.id === accountId) ?? list[0]
  const pickedFrom = value ? list.find((c) => c.id === value.connectionId) : undefined

  // The account of the picked repository was disconnected: nothing is picked anymore.
  React.useEffect(() => {
    if (value && connections.data && !connections.data.some((c) => c.id === value.connectionId)) onChange(null)
  }, [value, connections.data, onChange])

  const dialogNode = (
    <ConnectGitDialog
      open={dialog !== null}
      onOpenChange={(o) => !o && setDialog(null)}
      provider={dialog?.provider}
      connection={dialog?.connection}
      onConnected={(c) => {
        setAccountId(c.id)
        setChoosing(true)
      }}
    />
  )

  if (connections.isPending) return <Skeleton className="h-[120px] w-full" />
  if (connections.isError && !connections.data) {
    return (
      <ErrorState
        error={connections.error}
        title="Could not load the connected git accounts"
        onRetry={() => void connections.refetch()}
        retrying={connections.isRefetching}
      />
    )
  }

  if (!account) {
    const resume = unfinished[0]
    return (
      <>
        <EmptyState
          size="sm"
          title="Connect GitHub or GitLab"
          description="Authorize this server on GitHub or GitLab, then pick the repository from a list. Private repositories work too."
          actions={
            resume ? (
              <>
                <Button
                  size="md"
                  variant="primary"
                  icon={<GitProviderIcon provider={resume.provider} />}
                  onClick={() => setDialog({ connection: resume })}
                >
                  Finish connecting {GIT_PROVIDER_LABELS[resume.provider]}
                </Button>
                <Button size="md" icon={<Plus />} onClick={() => setDialog({})}>
                  Another account
                </Button>
              </>
            ) : (
              <ConnectButtons onConnect={(provider) => setDialog({ provider })} />
            )
          }
        >
          {/* The focus target of the field while there is nothing to pick from. */}
          <span id={id} tabIndex={-1} className="sr-only" aria-describedby={describedBy}>
            No git account is connected yet.
          </span>
        </EmptyState>
        {dialogNode}
      </>
    )
  }

  if (value && pickedFrom && !choosing) {
    return (
      <>
        <div
          className={cn(
            'flex items-center gap-3 rounded-md border bg-surface-100 px-3 py-2.5 dark:bg-surface-200/40',
            invalid && 'border-destructive',
          )}
        >
          <GitProviderIcon provider={pickedFrom.provider} className="size-[18px] shrink-0 text-foreground-light" />
          <div className="flex min-w-0 flex-1 flex-col">
            <span className="flex min-w-0 items-center gap-2">
              <span className="truncate font-mono text-[13px] text-foreground">{value.fullName}</span>
              {value.private && (
                <Badge variant="outline" case="normal">
                  <Lock aria-hidden="true" /> Private
                </Badge>
              )}
            </span>
            <span className="truncate text-[12.5px] text-foreground-lighter">
              From the {GIT_PROVIDER_LABELS[pickedFrom.provider]} account {connectionLabel(pickedFrom)}
            </span>
          </div>
          <Button id={id} size="tiny" aria-describedby={describedBy} onClick={() => setChoosing(true)}>
            Change
          </Button>
        </div>
        {dialogNode}
      </>
    )
  }

  return (
    <>
      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <Select value={account.id} onValueChange={setAccountId}>
            <SelectTrigger className="min-w-0 flex-1" aria-label="Git account">
              <SelectValue />
            </SelectTrigger>
            <SelectContent position="popper" align="start">
              {list.map((c) => (
                <SelectItem key={c.id} value={c.id}>
                  <GitProviderIcon provider={c.provider} className="size-3.5 text-foreground-light" />
                  <span className="truncate">{connectionLabel(c)}</span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button size="md" icon={<Plus />} onClick={() => setDialog({})}>
            Add account
          </Button>
        </div>
        <RepositoryList
          key={account.id}
          id={id}
          connection={account}
          value={value}
          invalid={invalid}
          describedBy={describedBy}
          onReconnect={() => setDialog({ connection: account })}
          onPick={(r) => {
            onChange(
              {
                connectionId: account.id,
                fullName: r.full_name,
                cloneUrl: r.clone_url,
                private: r.private,
                defaultBranch: r.default_branch,
              },
              r,
            )
            setChoosing(false)
          }}
        />
        <p className="text-[12.5px] text-foreground-lighter">
          Accounts are connected to this server, for every service: manage them in{' '}
          <Link
            to="/server?section=connections"
            target="_blank"
            className="rounded-sm text-primary underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
          >
            Server → Connections
          </Link>
          <span className="sr-only"> (opens in a new tab)</span>.
        </p>
      </div>
      {dialogNode}
    </>
  )
}
