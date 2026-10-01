import * as React from 'react'
import { AlertTriangle, KeyRound, LogOut, MonitorSmartphone, Plus, Trash2 } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { CodeBlock, CopyField } from '@/components/patterns/Copy'
import { Callout, EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { PageHeader, PageSection } from '@/components/patterns/Page'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { ApiError, errorMessage } from '@/lib/api/client'
import {
  useApiTokens,
  useChangePassword,
  useCreateApiToken,
  useEndSession,
  useRevokeApiToken,
  useSessions,
} from '@/lib/api/queries'
import { signOut } from '@/lib/api/session'
import type { ApiTokenView, CreatedApiToken, SessionView } from '@/lib/api/types'
import { relativeTime, shortDate } from '@/lib/format'
import { describeUserAgent } from '@/lib/userAgent'
import { Field, FormError, PasswordInput } from '@/pages/login/AuthLayout'
import { fieldAria, MIN_PASSWORD_LENGTH, passwordError } from '@/pages/login/forms'
import { useAuth } from '@/stores/auth'

import { RowValue } from './parts'

// ---------------------------------------------------------------------------
// password

function ChangePasswordDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const change = useChangePassword()
  const [current, setCurrent] = React.useState('')
  const [next, setNext] = React.useState('')
  const [touched, setTouched] = React.useState(false)
  const [currentError, setCurrentError] = React.useState<string | null>(null)
  const [error, setError] = React.useState<string | null>(null)
  const currentId = React.useId()
  const nextId = React.useId()

  const close = () => {
    onOpenChange(false)
    setCurrent('')
    setNext('')
    setTouched(false)
    setCurrentError(null)
    setError(null)
    change.reset()
  }

  const missingCurrent = current === '' ? 'Enter your current password.' : null
  const nextError = passwordError(next)
  const shownCurrentError = currentError ?? (touched ? missingCurrent : null)

  const submit = async () => {
    setTouched(true)
    if (missingCurrent || nextError || change.isPending) return
    setError(null)
    setCurrentError(null)
    try {
      await change.mutateAsync({ current_password: current, new_password: next })
      toast.success('Password changed', { description: 'Every other browser was signed out.' })
      close()
    } catch (e) {
      if (e instanceof ApiError && e.code === 'invalid_credentials') setCurrentError('This is not your current password.')
      else setError(errorMessage(e))
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (change.isPending) return
        if (o) onOpenChange(true)
        else close()
      }}
    >
      <DialogContent size="md">
        <form
          noValidate
          className="flex min-h-0 flex-col"
          onSubmit={(e) => {
            e.preventDefault()
            void submit()
          }}
        >
          <DialogHeader>
            <DialogTitle>Change password</DialogTitle>
            <DialogDescription>Every other browser signed in to this account is signed out.</DialogDescription>
          </DialogHeader>
          <DialogBody className="gap-5">
            <Field id={currentId} label="Current password" error={shownCurrentError}>
              <PasswordInput
                id={currentId}
                autoFocus
                autoComplete="current-password"
                value={current}
                onChange={(e) => {
                  setCurrent(e.target.value)
                  if (currentError) setCurrentError(null)
                }}
                {...fieldAria(currentId, null, shownCurrentError)}
              />
            </Field>
            <Field
              id={nextId}
              label="New password"
              hint={`At least ${MIN_PASSWORD_LENGTH} characters.`}
              error={touched ? nextError : null}
            >
              <PasswordInput
                id={nextId}
                autoComplete="new-password"
                value={next}
                onChange={(e) => setNext(e.target.value)}
                {...fieldAria(nextId, true, touched ? nextError : null)}
              />
            </Field>
            {error && <FormError>{error}</FormError>}
          </DialogBody>
          <DialogFooter>
            <Button disabled={change.isPending} onClick={close}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" loading={change.isPending}>
              Change password
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

// ---------------------------------------------------------------------------
// API tokens

const EXPIRY_OPTIONS = [
  { value: 'never', label: 'Never', days: null },
  { value: '30', label: '30 days', days: 30 },
  { value: '90', label: '90 days', days: 90 },
  { value: '365', label: '1 year', days: 365 },
] as const

/** "New API token" modal: a name and an expiry, then the token, once. */
function NewTokenDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const create = useCreateApiToken()
  const [name, setName] = React.useState('')
  const [expiry, setExpiry] = React.useState<string>('never')
  const [touched, setTouched] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const [created, setCreated] = React.useState<CreatedApiToken | null>(null)
  const nameId = React.useId()
  const expiryId = React.useId()

  const close = () => {
    onOpenChange(false)
    setName('')
    setExpiry('never')
    setTouched(false)
    setError(null)
    setCreated(null)
    create.reset()
  }

  const trimmed = name.trim()
  const nameError = trimmed === '' ? 'Give the token a name.' : trimmed.length > 100 ? 'Use at most 100 characters.' : null

  const submit = async () => {
    setTouched(true)
    if (nameError || create.isPending) return
    setError(null)
    try {
      const days = EXPIRY_OPTIONS.find((o) => o.value === expiry)?.days ?? null
      setCreated(await create.mutateAsync({ name: trimmed, expires_in_days: days }))
    } catch (e) {
      setError(errorMessage(e))
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (create.isPending) return
        if (o) onOpenChange(true)
        else close()
      }}
    >
      <DialogContent size="lg">
        {created ? (
          <>
            <DialogHeader>
              <DialogTitle>Token “{created.api_token.name}” created</DialogTitle>
              <DialogDescription>
                Copy it now. The server only keeps a fingerprint of it, so it can’t be shown again.
              </DialogDescription>
            </DialogHeader>
            <DialogBody className="gap-5">
              <CopyField value={created.token} what="API token" aria-label="API token" />
              <div className="flex flex-col gap-2">
                <span className="text-[13px] text-foreground-light">Use it with the CLI, for example in CI:</span>
                <CodeBlock code={`ferry login --server ${window.location.origin} --token <token>`} prompt />
              </div>
            </DialogBody>
            <DialogFooter>
              <Button variant="primary" onClick={close}>
                Done
              </Button>
            </DialogFooter>
          </>
        ) : (
          <form
            noValidate
            className="flex min-h-0 flex-col"
            onSubmit={(e) => {
              e.preventDefault()
              void submit()
            }}
          >
            <DialogHeader>
              <DialogTitle>New API token</DialogTitle>
              <DialogDescription>
                For a script, CI or a machine without a browser. It can do everything the API offers except manage
                this account and its tokens.
              </DialogDescription>
            </DialogHeader>
            <DialogBody className="gap-5">
              <Field
                id={nameId}
                label="Name"
                hint="What the token is for, so you know which one to revoke."
                error={touched ? nameError : null}
              >
                <Input
                  id={nameId}
                  autoFocus
                  autoComplete="off"
                  placeholder="CI"
                  maxLength={100}
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  {...fieldAria(nameId, true, touched ? nameError : null)}
                />
              </Field>
              <Field id={expiryId} label="Expires">
                <Select value={expiry} onValueChange={setExpiry}>
                  <SelectTrigger id={expiryId} className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {EXPIRY_OPTIONS.map((o) => (
                      <SelectItem key={o.value} value={o.value}>
                        {o.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Field>
              {error && <FormError>{error}</FormError>}
            </DialogBody>
            <DialogFooter>
              <Button disabled={create.isPending} onClick={close}>
                Cancel
              </Button>
              <Button type="submit" variant="primary" loading={create.isPending}>
                Create token
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  )
}

/** "expires in 3 days" / "expired 2 days ago" / null for a token without an end. */
function tokenExpiry(token: ApiTokenView, now: number = Date.now()): { label: string; expired: boolean } | null {
  if (!token.expires_at) return null
  const expired = new Date(token.expires_at).getTime() <= now
  return { label: `${expired ? 'expired' : 'expires'} ${relativeTime(token.expires_at)}`, expired }
}

function TokenRow({ token, onRevoke }: { token: ApiTokenView; onRevoke: () => void }) {
  const expiry = tokenExpiry(token)
  return (
    <li className="flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center md:px-6">
      <span
        aria-hidden="true"
        className="flex size-9 shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-light dark:bg-surface-200"
      >
        <KeyRound className="size-[18px]" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm">
          <span className="truncate font-medium text-foreground">{token.name}</span>
          <span className="font-mono text-[12.5px] text-foreground-lighter">fy_…{token.hint}</span>
          {expiry?.expired && (
            <Badge variant="warning" case="normal">
              Expired
            </Badge>
          )}
        </p>
        <p className="text-[12.5px] text-foreground-lighter">
          <span title={token.created_at}>Created {shortDate(token.created_at)}</span>
          {' · '}
          {token.last_used_at ? (
            <span title={token.last_used_at}>last used {relativeTime(token.last_used_at)}</span>
          ) : (
            'never used'
          )}
          {expiry && !expiry.expired && (
            <>
              {' · '}
              <span title={token.expires_at ?? undefined}>{expiry.label}</span>
            </>
          )}
        </p>
      </div>
      <Button size="tiny" variant="danger" icon={<Trash2 />} onClick={onRevoke} className="shrink-0 self-start sm:self-auto">
        Revoke
      </Button>
    </li>
  )
}

function ApiTokens() {
  const tokens = useApiTokens()
  const revoke = useRevokeApiToken()
  const [creating, setCreating] = React.useState(false)
  const [revoking, setRevoking] = React.useState<ApiTokenView | null>(null)
  const list = tokens.data ?? []

  return (
    <PageSection
      id="tokens"
      title="API tokens"
      description={
        <>
          What the CLI and scripts authenticate with. <code className="font-mono text-[13px]">ferry login</code> makes
          one for the terminal you approve; create others here for CI and machines without a browser.
        </>
      }
      actions={
        list.length > 0 ? (
          <Button size="sm" icon={<Plus />} onClick={() => setCreating(true)}>
            New token
          </Button>
        ) : undefined
      }
    >
      {tokens.isPending ? (
        <Skeleton className="h-[74px] w-full" />
      ) : tokens.isError && !tokens.data ? (
        <ErrorState
          error={tokens.error}
          title="Could not load the API tokens"
          onRetry={() => void tokens.refetch()}
          retrying={tokens.isRefetching}
        />
      ) : list.length === 0 ? (
        <EmptyState
          icon={<KeyRound />}
          title="No API token yet"
          description={
            <>
              Run <code className="font-mono text-[12.5px]">ferry login</code> in a terminal to connect it, or create a
              token for a script.
            </>
          }
          actions={
            <Button size="md" icon={<Plus />} onClick={() => setCreating(true)}>
              New token
            </Button>
          }
        />
      ) : (
        <Card>
          <ul className="divide-y" aria-label="API tokens">
            {list.map((t) => (
              <TokenRow key={t.id} token={t} onRevoke={() => setRevoking(t)} />
            ))}
          </ul>
        </Card>
      )}
      <p className="text-[12.5px] leading-relaxed text-foreground-lighter">
        The server also has a token of its own in <code className="font-mono">&lt;data-dir&gt;/api_token</code>, for
        scripts that run on the server itself. It is not listed here and can’t be revoked from the dashboard.
      </p>

      <NewTokenDialog open={creating} onOpenChange={setCreating} />
      <ConfirmDialog
        open={revoking !== null}
        onOpenChange={(o) => !o && setRevoking(null)}
        title={`Revoke “${revoking?.name ?? ''}”?`}
        description={<p>Whatever uses this token stops working at once. This can’t be undone.</p>}
        confirmLabel="Revoke token"
        onConfirm={() => {
          const t = revoking
          if (!t) return
          return revoke.mutateAsync(t.id).then(() => {
            toast.success(`Token ${t.name} revoked`)
          })
        }}
      />
    </PageSection>
  )
}

// ---------------------------------------------------------------------------
// sessions

function SessionRow({ session, onEnd }: { session: SessionView; onEnd: () => void }) {
  return (
    <li className="flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center md:px-6">
      <span
        aria-hidden="true"
        className="flex size-9 shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-light dark:bg-surface-200"
      >
        <MonitorSmartphone className="size-[18px]" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm">
          <span className="truncate font-medium text-foreground" title={session.user_agent ?? undefined}>
            {describeUserAgent(session.user_agent)}
          </span>
          {session.current && (
            <Badge variant="success" case="normal">
              This browser
            </Badge>
          )}
        </p>
        <p className="text-[12.5px] text-foreground-lighter">
          <span title={session.created_at}>Signed in {shortDate(session.created_at)}</span>
          {' · '}
          <span title={session.last_used_at}>
            {session.current ? 'active now' : `last active ${relativeTime(session.last_used_at)}`}
          </span>
        </p>
      </div>
      <Button size="tiny" icon={<LogOut />} onClick={onEnd} className="shrink-0 self-start sm:self-auto">
        Sign out
      </Button>
    </li>
  )
}

function Sessions() {
  const sessions = useSessions()
  const end = useEndSession()
  const list = sessions.data ?? []

  const endSession = (session: SessionView) => {
    if (session.current) {
      void signOut()
      return
    }
    end.mutate(session.id, {
      onSuccess: () => toast.success(`${describeUserAgent(session.user_agent)} signed out`),
      onError: (e) => toast.error('Could not sign that browser out', { description: errorMessage(e) }),
    })
  }

  return (
    <PageSection
      id="sessions"
      title="Sessions"
      description="The browsers signed in to this account. A session ends 30 days after it was last used."
    >
      {sessions.isPending ? (
        <Skeleton className="h-[74px] w-full" />
      ) : sessions.isError && !sessions.data ? (
        <ErrorState
          error={sessions.error}
          title="Could not load the sessions"
          onRetry={() => void sessions.refetch()}
          retrying={sessions.isRefetching}
        />
      ) : (
        <Card>
          <ul className="divide-y" aria-label="Sessions">
            {list.map((s) => (
              <SessionRow key={s.id} session={s} onEnd={() => endSession(s)} />
            ))}
          </ul>
        </Card>
      )}
    </PageSection>
  )
}

// ---------------------------------------------------------------------------

/**
 * Server → Account: the administrator of this server, the API tokens of the
 * CLI and of scripts, and the browsers signed in.
 */
export function AccountSection() {
  const user = useAuth((s) => s.user)
  const [changing, setChanging] = React.useState(false)
  return (
    <>
      <PageHeader
        title="Account"
        description="The account that administers this server, the API tokens of the CLI and of scripts, and the browsers signed in."
      />
      <PageSection title="Sign-in">
        <FormCard asDiv>
          <FormRow label="Email" description="What you sign in to this dashboard with.">
            <RowValue>{user?.email ?? ''}</RowValue>
          </FormRow>
          <FormRow
            label="Password"
            description={
              <>
                Forgot it? Run <code className="font-mono text-[12.5px]">ferryd reset-password</code> on the server.
              </>
            }
          >
            <RowValue>
              <Button size="sm" onClick={() => setChanging(true)}>
                Change password
              </Button>
            </RowValue>
          </FormRow>
        </FormCard>
        <Callout tone="info" icon={<AlertTriangle />} title="One account per server">
          Whoever signs in with it can deploy code and read every environment variable. Keep the dashboard on
          localhost or behind HTTPS.
        </Callout>
      </PageSection>
      <ApiTokens />
      <Sessions />
      <ChangePasswordDialog open={changing} onOpenChange={setChanging} />
    </>
  )
}
