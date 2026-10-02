import * as React from 'react'
import { X } from 'lucide-react'

import { ConnectGitDialog } from '@/components/git/ConnectGitDialog'
import { GitProviderIcon } from '@/components/patterns/icons'
import { Button } from '@/components/ui/button'
import { Hint } from '@/components/ui/tooltip'
import { useGitConnections } from '@/lib/api/queries'
import { GIT_PROVIDERS, type GitConnectionView, type GitProvider } from '@/lib/api/types'
import { connectedAccounts, GIT_PROVIDER_LABELS } from '@/lib/git'
import { safeStorage } from '@/lib/storage'

/** Set once the user said "not now" (per browser). */
const DISMISSED_KEY = 'ferry.git.prompt-dismissed'

type DialogState = { provider?: GitProvider; connection?: GitConnectionView } | null

/**
 * The home page asks for a GitHub / GitLab account as long as this server
 * has none: connecting one is what lets services be created from a list of
 * repositories. It goes away once an account is connected, or when dismissed.
 */
export function ConnectGitPrompt() {
  const connections = useGitConnections()
  const [dismissed, setDismissed] = React.useState(() => safeStorage.get(DISMISSED_KEY) === '1')
  const [dialog, setDialog] = React.useState<DialogState>(null)

  const dialogNode = (
    <ConnectGitDialog
      open={dialog !== null}
      onOpenChange={(o) => !o && setDialog(null)}
      provider={dialog?.provider}
      connection={dialog?.connection}
    />
  )

  // Nothing until the accounts are known: no flash for servers that have one.
  if (!connections.data || dismissed) return dialog ? dialogNode : null
  if (connectedAccounts(connections.data).length > 0) return dialog ? dialogNode : null
  // An authorization that was started and left half-way is finished first.
  const unfinished = connections.data.find((c) => c.status === 'pending')

  return (
    <section
      aria-labelledby="connect-git-title"
      className="relative flex flex-col gap-4 rounded-lg border bg-surface-100 p-5 sm:flex-row sm:gap-5 dark:bg-surface-200/40"
    >
      <div aria-hidden="true" className="flex shrink-0 items-start gap-2 text-foreground-light">
        {GIT_PROVIDERS.map((p) => (
          <span key={p} className="flex size-10 items-center justify-center rounded-md border bg-background">
            <GitProviderIcon provider={p} className="size-5" />
          </span>
        ))}
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="flex flex-col gap-1 pr-8">
          <h2 id="connect-git-title" className="text-sm font-medium text-foreground">
            {unfinished
              ? `Finish connecting ${GIT_PROVIDER_LABELS[unfinished.provider]}`
              : 'Connect GitHub or GitLab to deploy from your repositories'}
          </h2>
          <p className="text-[13px] leading-relaxed text-foreground-light">
            Authorize this server once: new services then pick their repository and branch from a list, private
            repositories included.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {unfinished ? (
            <Button
              variant="primary"
              icon={<GitProviderIcon provider={unfinished.provider} />}
              onClick={() => setDialog({ connection: unfinished })}
            >
              Finish connecting
            </Button>
          ) : (
            GIT_PROVIDERS.map((p) => (
              <Button key={p} icon={<GitProviderIcon provider={p} />} onClick={() => setDialog({ provider: p })}>
                Connect {GIT_PROVIDER_LABELS[p]}
              </Button>
            ))
          )}
        </div>
      </div>
      <Hint label="Not now">
        <Button
          size="icon-tiny"
          variant="ghost"
          aria-label="Not now: hide this"
          className="absolute top-2.5 right-2.5"
          icon={<X />}
          onClick={() => {
            safeStorage.set(DISMISSED_KEY, '1')
            setDismissed(true)
          }}
        />
      </Hint>
      {dialogNode}
    </section>
  )
}
