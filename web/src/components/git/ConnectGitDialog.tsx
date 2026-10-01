import * as React from 'react'
import { AlertTriangle, ChevronDown, ExternalLink } from 'lucide-react'
import { RadioGroup as RadioGroupPrimitive } from 'radix-ui'
import { toast } from 'sonner'

import { CopyField } from '@/components/patterns/Copy'
import { GitProviderIcon } from '@/components/patterns/icons'
import { Button } from '@/components/ui/button'
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
import { errorMessage } from '@/lib/api/client'
import { useConnectGit, useGitConnections } from '@/lib/api/queries'
import { GIT_PROVIDERS, type GitConnectionView, type GitProvider } from '@/lib/api/types'
import {
  accountName,
  applicationsPageUrl,
  GIT_DEFAULT_BASE_URLS,
  GIT_PROVIDER_LABELS,
  GITLAB_APPLICATION_SCOPES,
  instanceUrl,
  instanceUrlError,
  savedApplication,
  tokenPageUrl,
} from '@/lib/git'
import { gitRedirectUri } from '@/lib/gitAuthorize'
import { cn } from '@/lib/utils'

import { type GitAuthorizationFlow, useGitAuthorization } from './useGitAuthorization'

const TOKEN_HELP: Record<GitProvider, React.ReactNode> = {
  github: (
    <>
      A classic token with the <Code>repo</Code> scope (the link selects it). A fine-grained token works too, with read
      access to Contents and Metadata.
    </>
  ),
  gitlab: (
    <>
      A personal access token with the <Code>read_api</Code> and <Code>read_repository</Code> scopes (the link selects
      them).
    </>
  ),
}

const TOKEN_PLACEHOLDER: Record<GitProvider, string> = { github: 'ghp_…', gitlab: 'glpat-…' }

function Code({ children }: { children: React.ReactNode }) {
  return <code className="font-mono text-[12px] text-foreground-light">{children}</code>
}

interface ConnectGitDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Provider selected when the dialog opens (default GitHub). */
  provider?: GitProvider
  /**
   * Act on this connection instead of a new account: authorize it again
   * (or finish what was started), or give it another token. Its provider
   * and instance are kept.
   */
  connection?: GitConnectionView
  onConnected?: (connection: GitConnectionView) => void
}

/**
 * Connect a GitHub / GitLab account: the button sends the browser to the
 * provider's own authorization pages (GitHub registers an app for this
 * server and asks which repositories it may read; GitLab asks to authorize
 * the server's OAuth application), which send it back to the page this
 * dialog is on. Pasting an access token stays possible, as the other way.
 */
export function ConnectGitDialog({ open, onOpenChange, onConnected, ...props }: ConnectGitDialogProps) {
  const connect = useConnectGit()
  const close = () => onOpenChange(false)
  const flow = useGitAuthorization((connection) => {
    close()
    onConnected?.(connection)
  })
  const busy = connect.isPending || flow.phase !== 'idle'
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent size="lg">
        {/* Mounted while open: every opening starts from its props, with empty fields. */}
        <ConnectGitForm connect={connect} flow={flow} onClose={close} onConnected={onConnected} {...props} />
      </DialogContent>
    </Dialog>
  )
}

function ConnectGitForm({
  connect,
  flow,
  onClose,
  provider: initial,
  connection,
  onConnected,
}: Pick<ConnectGitDialogProps, 'provider' | 'connection' | 'onConnected'> & {
  connect: ReturnType<typeof useConnectGit>
  flow: GitAuthorizationFlow
  onClose: () => void
}) {
  const ids = {
    provider: React.useId(),
    token: React.useId(),
    instance: React.useId(),
    organization: React.useId(),
    appId: React.useId(),
    appKey: React.useId(),
    redirect: React.useId(),
  }
  const connections = useGitConnections()
  const [provider, setProvider] = React.useState<GitProvider>(connection?.provider ?? initial ?? 'github')
  // A connection made with a token is given another token; everything else goes through the provider.
  const [withToken, setWithToken] = React.useState(connection?.auth === 'token')
  const [token, setToken] = React.useState('')
  const [instance, setInstance] = React.useState('')
  const [organization, setOrganization] = React.useState('')
  const [options, setOptions] = React.useState(false)
  const [appId, setAppId] = React.useState('')
  const [appKey, setAppKey] = React.useState('')
  const [otherApplication, setOtherApplication] = React.useState(false)
  const [submitted, setSubmitted] = React.useState(false)
  const [tokenFailure, setTokenFailure] = React.useState<string | null>(null)

  const label = GIT_PROVIDER_LABELS[provider]
  const baseUrl = connection ? connection.base_url : instanceUrl(instance)
  const custom = baseUrl && baseUrl !== GIT_DEFAULT_BASE_URLS[provider] ? baseUrl : undefined
  const instanceError = connection ? null : instanceUrlError(instance)
  const pending = connect.isPending || flow.phase !== 'idle'

  // GitLab: the application this server already has for that instance, if any.
  const application =
    provider === 'gitlab'
      ? connection?.auth === 'oauth' && connection.client_id
        ? connection
        : savedApplication(connections.data, 'gitlab', baseUrl)
      : undefined
  // The server may know better (the list is still loading, an application was removed).
  const applicationMissing = flow.errorCode === 'git_application_required'
  const needsApplication = provider === 'gitlab' && (!application || otherApplication || applicationMissing)

  const trimmedToken = token.trim()
  const tokenError = !trimmedToken
    ? 'Paste the access token.'
    : /\s/.test(trimmedToken)
      ? 'A token has no spaces: check what was pasted.'
      : null
  const organizationError =
    organization.trim() && !/^[a-z0-9](?:[a-z0-9-]{0,37}[a-z0-9])?$/i.test(organization.trim())
      ? 'Enter the organization’s GitHub login (letters, digits and “-”).'
      : null
  const appIdError = needsApplication && !appId.trim() ? 'Paste the Application ID.' : null
  const appKeyError = needsApplication && !appKey.trim() ? 'Paste the Secret.' : null

  const errors = withToken
    ? [tokenError, instanceError]
    : provider === 'github'
      ? [instanceError, organizationError]
      : [instanceError, appIdError, appKeyError]
  const valid = errors.every((e) => e === null)
  const shown = (error: string | null, touched = false) => (submitted || touched ? error : null)
  const failure = withToken ? tokenFailure : applicationMissing ? null : flow.error
  const optionsOpen = options || instance.trim() !== '' || organization.trim() !== ''

  const reset = () => {
    setSubmitted(false)
    setTokenFailure(null)
    flow.clearError()
  }

  const submit = () => {
    setSubmitted(true)
    if (!valid || pending) return
    setTokenFailure(null)
    if (withToken) {
      connect.mutate(
        { provider, token: trimmedToken, ...(custom ? { base_url: custom } : {}) },
        {
          onSuccess: (c) => {
            toast.success(
              connection ? `Token of ${c.account} replaced` : `${GIT_PROVIDER_LABELS[c.provider]} account ${c.account} connected`,
            )
            onClose()
            onConnected?.(c)
          },
          onError: (e) => setTokenFailure(errorMessage(e)),
        },
      )
      return
    }
    const newApplication = needsApplication ? { client_id: appId.trim(), client_secret: appKey.trim() } : {}
    if (connection && connection.auth !== 'token') {
      flow.start({ connection_id: connection.id, ...newApplication })
    } else {
      flow.start({
        provider,
        ...(custom ? { base_url: custom } : {}),
        ...(provider === 'github' && organization.trim() ? { organization: organization.trim() } : {}),
        ...newApplication,
      })
    }
  }

  const title = !connection
    ? 'Connect a git account'
    : withToken
      ? `Replace the token of ${accountName(connection)}`
      : connection.status === 'pending'
        ? `Finish connecting ${label}`
        : `Authorize ${accountName(connection)} again`

  return (
    <form
      noValidate
      className="flex min-h-0 flex-col"
      onSubmit={(e) => {
        e.preventDefault()
        // The dialog may sit inside another form (new service).
        e.stopPropagation()
        submit()
      }}
    >
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>
          {connection && withToken
            ? `Repositories on ${label} are cloned with the new token from the next deploy.`
            : 'Ferry lists the account’s repositories and clones them, private ones included. The authorization belongs to this server: every service uses it.'}
        </DialogDescription>
      </DialogHeader>
      <DialogBody className="gap-5">
        {!connection && (
          <div className="flex flex-col gap-1.5">
            <span id={ids.provider} className="text-[13px] font-medium text-foreground">
              Provider
            </span>
            <RadioGroupPrimitive.Root
              aria-labelledby={ids.provider}
              value={provider}
              onValueChange={(v) => {
                const next = GIT_PROVIDERS.find((p) => p === v)
                if (next) setProvider(next)
                reset()
              }}
              className="grid grid-cols-2 gap-2"
            >
              {GIT_PROVIDERS.map((p) => (
                <RadioGroupPrimitive.Item
                  key={p}
                  value={p}
                  className={cn(
                    'group flex cursor-pointer items-center gap-2.5 rounded-lg border bg-surface-100 px-3 py-2.5 text-left outline-none transition-colors dark:bg-surface-200',
                    'hover:border-border-stronger focus-visible:ring-2 focus-visible:ring-ring',
                    'data-[state=checked]:border-primary-bright/70 data-[state=checked]:bg-primary-soft',
                  )}
                >
                  <GitProviderIcon
                    provider={p}
                    className="size-[18px] text-foreground-light group-data-[state=checked]:text-foreground"
                  />
                  <span className="text-sm font-medium text-foreground">{GIT_PROVIDER_LABELS[p]}</span>
                </RadioGroupPrimitive.Item>
              ))}
            </RadioGroupPrimitive.Root>
          </div>
        )}

        {withToken ? (
          <div className="flex flex-col gap-1.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <label htmlFor={ids.token} className="text-[13px] font-medium text-foreground">
                Access token
              </label>
              <Button asChild size="tiny" variant="outline" iconRight={<ExternalLink />}>
                <a href={tokenPageUrl(provider, baseUrl)} target="_blank" rel="noreferrer">
                  Create a token on {label}
                </a>
              </Button>
            </div>
            <Input
              id={ids.token}
              type="password"
              mono
              autoFocus
              autoComplete="off"
              spellCheck={false}
              data-1p-ignore
              data-lpignore="true"
              placeholder={TOKEN_PLACEHOLDER[provider]}
              value={token}
              onChange={(e) => {
                setToken(e.target.value)
                setTokenFailure(null)
              }}
              aria-invalid={shown(tokenError) ? true : undefined}
              aria-describedby={`${ids.token}-${shown(tokenError) ? 'error' : 'hint'}`}
            />
            {shown(tokenError) ? (
              <p id={`${ids.token}-error`} role="alert" className="text-[12.5px] text-destructive">
                {tokenError}
              </p>
            ) : (
              <p id={`${ids.token}-hint`} className="text-[12.5px] leading-relaxed text-foreground-lighter">
                {TOKEN_HELP[provider]} It is stored on this server and never shown again.
              </p>
            )}
          </div>
        ) : provider === 'github' ? (
          <ol className="flex flex-col gap-2.5 text-[13px] leading-relaxed text-foreground-light">
            <Step n={1}>
              {connection?.status === 'pending' ? (
                <>
                  The GitHub App <Code>{connection.app_slug}</Code> is registered for this server.
                </>
              ) : (
                <>GitHub registers a private app for this Ferry server on your account: confirm its name there.</>
              )}
            </Step>
            <Step n={2}>
              You choose the repositories the app may read. GitHub sends you back here, and no token is ever typed.
            </Step>
          </ol>
        ) : needsApplication ? (
          <div className="flex flex-col gap-4">
            <ol className="flex flex-col gap-2.5 text-[13px] leading-relaxed text-foreground-light">
              <Step n={1}>
                <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
                  Create an application for this server on GitLab, once:
                  <Button asChild size="tiny" variant="outline" iconRight={<ExternalLink />}>
                    <a href={applicationsPageUrl(baseUrl)} target="_blank" rel="noreferrer">
                      Open GitLab applications
                    </a>
                  </Button>
                </div>
                <div className="mt-2 flex flex-col gap-1.5">
                  <label htmlFor={ids.redirect} className="text-[12.5px] text-foreground-lighter">
                    Redirect URI
                  </label>
                  <CopyField id={ids.redirect} value={gitRedirectUri()} what="Redirect URI" size="sm" />
                  <p className="text-[12.5px] text-foreground-lighter">
                    Scopes:{' '}
                    {GITLAB_APPLICATION_SCOPES.map((s, i) => (
                      <React.Fragment key={s}>
                        {i > 0 && ', '}
                        <Code>{s}</Code>
                      </React.Fragment>
                    ))}
                    . Keep <span className="text-foreground-light">Confidential</span> checked.
                  </p>
                </div>
              </Step>
              <Step n={2}>Paste what GitLab shows for the application, then authorize it.</Step>
            </ol>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field id={ids.appId} label="Application ID" error={shown(appIdError)}>
                <Input
                  id={ids.appId}
                  mono
                  autoComplete="off"
                  spellCheck={false}
                  value={appId}
                  onChange={(e) => {
                    setAppId(e.target.value)
                    flow.clearError()
                  }}
                  aria-invalid={shown(appIdError) ? true : undefined}
                  aria-describedby={shown(appIdError) ? `${ids.appId}-error` : undefined}
                />
              </Field>
              <Field id={ids.appKey} label="Secret" error={shown(appKeyError)}>
                <Input
                  id={ids.appKey}
                  type="password"
                  mono
                  autoComplete="off"
                  spellCheck={false}
                  data-1p-ignore
                  data-lpignore="true"
                  value={appKey}
                  onChange={(e) => {
                    setAppKey(e.target.value)
                    flow.clearError()
                  }}
                  aria-invalid={shown(appKeyError) ? true : undefined}
                  aria-describedby={shown(appKeyError) ? `${ids.appKey}-error` : undefined}
                />
              </Field>
            </div>
            {application && !applicationMissing && (
              <button type="button" onClick={() => setOtherApplication(false)} className={LINK}>
                Keep the application already saved
              </button>
            )}
          </div>
        ) : (
          <div className="flex flex-col gap-2 text-[13px] leading-relaxed text-foreground-light">
            <p>
              GitLab asks you to authorize this server’s application
              {application?.client_id ? (
                <>
                  {' '}
                  (<Code>…{application.client_id.slice(-6)}</Code>)
                </>
              ) : null}
              , then sends you back here. No token is ever typed.
            </p>
            <button type="button" onClick={() => setOtherApplication(true)} className={LINK}>
              Use another application
            </button>
          </div>
        )}

        {!connection && (
          <div className="flex flex-col gap-2">
            <button
              type="button"
              aria-expanded={optionsOpen}
              aria-controls={`${ids.instance}-panel`}
              // What was typed stays in sight: it decides where the browser is sent.
              disabled={instance.trim() !== '' || organization.trim() !== ''}
              onClick={() => setOptions((o) => !o)}
              className="flex w-fit items-center gap-1 rounded-sm text-[13px] text-foreground-light outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:hover:text-foreground-light"
            >
              <ChevronDown className={cn('size-3.5 transition-transform', !optionsOpen && '-rotate-90')} aria-hidden="true" />
              {provider === 'github' && !withToken ? 'Organization, self-hosted GitHub' : `Self-hosted ${label}`}
            </button>
            <div id={`${ids.instance}-panel`} hidden={!optionsOpen} className="flex flex-col gap-4">
              {provider === 'github' && !withToken && (
                <Field
                  id={ids.organization}
                  label="Organization"
                  error={shown(organizationError, organization.trim() !== '')}
                  hint="To deploy an organization’s repositories, the app is registered in the organization: enter its login. Leave empty for your own account."
                >
                  <Input
                    id={ids.organization}
                    mono
                    autoComplete="off"
                    spellCheck={false}
                    placeholder="acme"
                    value={organization}
                    onChange={(e) => {
                      setOrganization(e.target.value)
                      flow.clearError()
                    }}
                    aria-invalid={shown(organizationError, organization.trim() !== '') ? true : undefined}
                    aria-describedby={`${ids.organization}-${shown(organizationError, organization.trim() !== '') ? 'error' : 'hint'}`}
                  />
                </Field>
              )}
              <Field
                id={ids.instance}
                label="Instance address"
                error={shown(instanceError, instance.trim() !== '')}
                hint={
                  <>
                    {provider === 'github' ? 'GitHub Enterprise Server.' : 'GitLab self-managed.'} Leave empty for{' '}
                    {GIT_DEFAULT_BASE_URLS[provider].replace('https://', '')}.
                  </>
                }
              >
                <Input
                  id={ids.instance}
                  mono
                  autoComplete="off"
                  spellCheck={false}
                  placeholder={provider === 'github' ? 'https://github.example.com' : 'https://gitlab.example.com'}
                  value={instance}
                  onChange={(e) => {
                    setInstance(e.target.value)
                    reset()
                  }}
                  aria-invalid={shown(instanceError, instance.trim() !== '') ? true : undefined}
                  aria-describedby={`${ids.instance}-${shown(instanceError, instance.trim() !== '') ? 'error' : 'hint'}`}
                />
              </Field>
            </div>
          </div>
        )}

        {failure && (
          <div
            role="alert"
            className="flex items-start gap-2 rounded-md border border-destructive-border bg-destructive-soft p-3 text-[13px] text-foreground"
          >
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
            <span className="break-words">{failure}</span>
          </div>
        )}

        {(!connection || connection.auth === 'token') && (
          <button
            type="button"
            className={LINK}
            onClick={() => {
              setWithToken((t) => !t)
              reset()
            }}
          >
            {withToken ? `Authorize on ${label} instead` : 'Use an access token instead'}
          </button>
        )}
      </DialogBody>
      <DialogFooter>
        <Button disabled={pending} onClick={onClose}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" loading={pending} disabled={submitted && !valid}>
          {withToken ? (connection ? 'Replace token' : `Connect ${label}`) : `Continue to ${label}`}
        </Button>
      </DialogFooter>
    </form>
  )
}

const LINK =
  'w-fit rounded-sm text-[13px] text-foreground-light underline underline-offset-4 outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring'

function Step({ n, children }: { n: number; children: React.ReactNode }) {
  return (
    <li className="flex gap-2.5">
      <span
        aria-hidden="true"
        className="mt-px flex size-5 shrink-0 items-center justify-center rounded-full border bg-surface-100 text-[11px] font-medium text-foreground-light tabular-nums dark:bg-surface-200"
      >
        {n}
      </span>
      <div className="min-w-0 flex-1">{children}</div>
    </li>
  )
}

function Field({
  id,
  label,
  error,
  hint,
  children,
}: {
  id: string
  label: string
  error?: string | null
  hint?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <label htmlFor={id} className="text-[13px] font-medium text-foreground">
        {label}
      </label>
      {children}
      {error ? (
        <p id={`${id}-error`} role="alert" className="text-[12.5px] text-destructive">
          {error}
        </p>
      ) : hint ? (
        <p id={`${id}-hint`} className="text-[12.5px] leading-relaxed text-foreground-lighter">
          {hint}
        </p>
      ) : null}
    </div>
  )
}
