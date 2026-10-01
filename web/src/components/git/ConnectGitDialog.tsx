import * as React from 'react'
import { AlertTriangle, ChevronDown, ExternalLink } from 'lucide-react'
import { RadioGroup as RadioGroupPrimitive } from 'radix-ui'
import { toast } from 'sonner'

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
import { useConnectGit } from '@/lib/api/queries'
import { GIT_PROVIDERS, type GitConnectionView, type GitProvider } from '@/lib/api/types'
import { GIT_DEFAULT_BASE_URLS, GIT_PROVIDER_LABELS, instanceUrl, instanceUrlError, tokenPageUrl } from '@/lib/git'
import { cn } from '@/lib/utils'

const TOKEN_HELP: Record<GitProvider, React.ReactNode> = {
  github: (
    <>
      A classic token with the <code className="font-mono text-[12px]">repo</code> scope (the link selects it). A
      fine-grained token works too, with read access to Contents and Metadata.
    </>
  ),
  gitlab: (
    <>
      A personal access token with the <code className="font-mono text-[12px]">read_api</code> and{' '}
      <code className="font-mono text-[12px]">read_repository</code> scopes (the link selects them).
    </>
  ),
}

const TOKEN_PLACEHOLDER: Record<GitProvider, string> = { github: 'ghp_…', gitlab: 'glpat-…' }

interface ConnectGitDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Provider selected when the dialog opens (default GitHub). */
  provider?: GitProvider
  /** Replace the token of this connection: its provider and instance are kept. */
  connection?: GitConnectionView
  onConnected?: (connection: GitConnectionView) => void
}

/**
 * Connect a GitHub / GitLab account with a personal access token: pick the
 * provider, create the token on its site (link with the scopes filled in),
 * paste it. Ferry asks the provider whose token it is.
 */
export function ConnectGitDialog({ open, onOpenChange, ...props }: ConnectGitDialogProps) {
  const connect = useConnectGit()
  return (
    <Dialog open={open} onOpenChange={(o) => !connect.isPending && onOpenChange(o)}>
      <DialogContent size="lg">
        {/* Mounted while open: every opening starts from its props, with an empty token. */}
        <ConnectGitForm connect={connect} onClose={() => onOpenChange(false)} {...props} />
      </DialogContent>
    </Dialog>
  )
}

function ConnectGitForm({
  connect,
  onClose,
  provider: initial,
  connection,
  onConnected,
}: Pick<ConnectGitDialogProps, 'provider' | 'connection' | 'onConnected'> & {
  connect: ReturnType<typeof useConnectGit>
  onClose: () => void
}) {
  const ids = { provider: React.useId(), token: React.useId(), instance: React.useId() }
  const [provider, setProvider] = React.useState<GitProvider>(connection?.provider ?? initial ?? 'github')
  const [token, setToken] = React.useState('')
  const [instance, setInstance] = React.useState('')
  const [selfHosted, setSelfHosted] = React.useState(false)
  const [submitted, setSubmitted] = React.useState(false)
  const [submitError, setSubmitError] = React.useState<string | null>(null)

  const label = GIT_PROVIDER_LABELS[provider]
  const baseUrl = connection ? connection.base_url : instanceUrl(instance)
  const trimmed = token.trim()
  const tokenError = !trimmed
    ? 'Paste the access token.'
    : /\s/.test(trimmed)
      ? 'A token has no spaces: check what was pasted.'
      : null
  const instanceError = connection ? null : instanceUrlError(instance)
  const valid = !tokenError && !instanceError
  const shownTokenError = submitted ? tokenError : null
  const shownInstanceError = submitted || instance.trim() !== '' ? instanceError : null
  const panelOpen = selfHosted || instance.trim() !== ''

  const submit = () => {
    setSubmitted(true)
    if (!valid || connect.isPending) return
    setSubmitError(null)
    const custom = baseUrl && baseUrl !== GIT_DEFAULT_BASE_URLS[provider] ? baseUrl : undefined
    connect.mutate(
      { provider, token: trimmed, ...(custom ? { base_url: custom } : {}) },
      {
        onSuccess: (c) => {
          toast.success(
            connection
              ? `Token of ${c.account} replaced`
              : `${GIT_PROVIDER_LABELS[c.provider]} account ${c.account} connected`,
          )
          onClose()
          onConnected?.(c)
        },
        onError: (e) => setSubmitError(errorMessage(e)),
      },
    )
  }

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
        <DialogTitle>{connection ? `Replace the token of ${connection.account}` : 'Connect a git account'}</DialogTitle>
        <DialogDescription>
          {connection
            ? `The services using this ${label} account clone with the new token from their next deploy.`
            : 'Ferry lists the account’s repositories and clones them with its token, private ones included.'}
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
                setSubmitError(null)
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
              setSubmitError(null)
            }}
            aria-invalid={shownTokenError ? true : undefined}
            aria-describedby={`${ids.token}-${shownTokenError ? 'error' : 'hint'}`}
          />
          {shownTokenError ? (
            <p id={`${ids.token}-error`} role="alert" className="text-[12.5px] text-destructive">
              {shownTokenError}
            </p>
          ) : (
            <p id={`${ids.token}-hint`} className="text-[12.5px] leading-relaxed text-foreground-lighter">
              {TOKEN_HELP[provider]} It is stored on this server and never shown again.
            </p>
          )}
        </div>

        {!connection && (
          <div className="flex flex-col gap-2">
            <button
              type="button"
              aria-expanded={panelOpen}
              aria-controls={`${ids.instance}-panel`}
              // An address that was typed stays in sight: it decides where the token is sent.
              disabled={instance.trim() !== ''}
              onClick={() => setSelfHosted((o) => !o)}
              className="flex w-fit items-center gap-1 rounded-sm text-[13px] text-foreground-light outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:hover:text-foreground-light"
            >
              <ChevronDown className={cn('size-3.5 transition-transform', !panelOpen && '-rotate-90')} aria-hidden="true" />
              Self-hosted {label}
            </button>
            <div id={`${ids.instance}-panel`} hidden={!panelOpen} className="flex flex-col gap-1.5">
              <label htmlFor={ids.instance} className="text-[13px] font-medium text-foreground">
                Instance address
              </label>
              <Input
                id={ids.instance}
                mono
                autoComplete="off"
                spellCheck={false}
                placeholder={provider === 'github' ? 'https://github.example.com' : 'https://gitlab.example.com'}
                value={instance}
                onChange={(e) => {
                  setInstance(e.target.value)
                  setSubmitError(null)
                }}
                aria-invalid={shownInstanceError ? true : undefined}
                aria-describedby={`${ids.instance}-${shownInstanceError ? 'error' : 'hint'}`}
              />
              {shownInstanceError ? (
                <p id={`${ids.instance}-error`} role="alert" className="text-[12.5px] text-destructive">
                  {shownInstanceError}
                </p>
              ) : (
                <p id={`${ids.instance}-hint`} className="text-[12.5px] text-foreground-lighter">
                  {provider === 'github' ? 'GitHub Enterprise Server.' : 'GitLab self-managed.'} Leave empty for{' '}
                  {GIT_DEFAULT_BASE_URLS[provider].replace('https://', '')}.
                </p>
              )}
            </div>
          </div>
        )}

        {submitError && (
          <div
            role="alert"
            className="flex items-start gap-2 rounded-md border border-destructive-border bg-destructive-soft p-3 text-[13px] text-foreground"
          >
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
            <span className="break-words">{submitError}</span>
          </div>
        )}
      </DialogBody>
      <DialogFooter>
        <Button disabled={connect.isPending} onClick={onClose}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" loading={connect.isPending} disabled={submitted && !valid}>
          {connection ? 'Replace token' : `Connect ${label}`}
        </Button>
      </DialogFooter>
    </form>
  )
}
