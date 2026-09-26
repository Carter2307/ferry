import * as React from 'react'
import { ExternalLink, Globe, Plus, Trash2 } from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { CopyButton } from '@/components/patterns/Copy'
import { FormCard } from '@/components/patterns/FormCard'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { PageSection } from '@/components/patterns/Page'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Hint } from '@/components/ui/tooltip'
import { errorMessage } from '@/lib/api/client'
import { useAddDomain, useRemoveDomain, useServerInfo, useServiceDomains } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'

import { checkDomain } from './forms'

/** `http://api.localhost:19801` + `www.example.com` → `http://www.example.com:19801`. */
function hostUrl(serviceUrl: string | null, host: string): string | null {
  if (!serviceUrl) return null
  try {
    const u = new URL(serviceUrl)
    return `${u.protocol}//${host}${u.port ? `:${u.port}` : ''}`
  } catch {
    return null
  }
}

/** Custom domains (add / remove) and every host the proxy routes to the service. */
export function DomainsSection({ service }: { service: ServiceView }) {
  const name = service.name
  const domainsQuery = useServiceDomains(name)
  const add = useAddDomain(name)
  const remove = useRemoveDomain(name)
  const info = useServerInfo()
  const [input, setInput] = React.useState('')
  const [touched, setTouched] = React.useState(false)
  const [serverError, setServerError] = React.useState<string | null>(null)
  const [toRemove, setToRemove] = React.useState<string | null>(null)
  const inputId = React.useId()
  const errorId = `${inputId}-error`

  const custom = domainsQuery.data ?? service.custom_domains
  const defaults = service.hosts.filter((h) => !service.custom_domains.includes(h))
  const hosts = [...defaults.map((h) => ({ host: h, custom: false })), ...custom.map((h) => ({ host: h, custom: true }))]

  const check = checkDomain(input)
  const duplicate = !check.error && (custom.includes(check.domain) || defaults.includes(check.domain))
  const clientError = !input.trim() ? null : check.error ?? (duplicate ? `${check.domain} is already routed to ${name}.` : null)
  const shownError = (touched ? clientError : null) ?? serverError

  const onAdd = (e: React.FormEvent) => {
    e.preventDefault()
    setTouched(true)
    setServerError(null)
    if (!input.trim()) {
      setServerError('Enter a domain.')
      return
    }
    if (clientError) return
    const domain = check.domain
    add.mutate(domain, {
      onSuccess: () => {
        toast.success(`Added ${domain}`, { description: `The proxy now routes it to ${name}.` })
        setInput('')
        setTouched(false)
      },
      onError: (err) => setServerError(errorMessage(err)),
    })
  }

  return (
    <PageSection
      id="domains"
      title="Custom domains"
      description={
        <>
          Serve {name} on your own domains. Point their DNS (A/AAAA or CNAME) at this server
          {info.data?.tls_enabled ? '; certificates are issued automatically.' : '.'}
        </>
      }
    >
      <FormCard asDiv aria-label="Custom domains">
        <form onSubmit={onAdd} noValidate className="flex flex-col gap-2 px-5 py-4 md:px-6">
          <label htmlFor={inputId} className="text-sm font-medium text-foreground">
            Add a domain
          </label>
          <div className="flex flex-col gap-2 sm:flex-row">
            <Input
              id={inputId}
              mono
              placeholder="app.example.com"
              autoComplete="off"
              spellCheck={false}
              autoCapitalize="none"
              value={input}
              aria-invalid={shownError ? true : undefined}
              aria-describedby={shownError ? errorId : undefined}
              onChange={(e) => {
                setInput(e.target.value)
                setServerError(null)
              }}
              onBlur={() => input.trim() && setTouched(true)}
            />
            <Button type="submit" size="md" icon={<Plus />} loading={add.isPending} className="shrink-0">
              Add domain
            </Button>
          </div>
          {shownError && (
            <p id={errorId} role="alert" className="text-[13px] text-destructive">
              {shownError}
            </p>
          )}
        </form>

        <div className="px-5 pt-4 pb-1 md:px-6">
          <MonoLabel as="h3">Routed hosts</MonoLabel>
        </div>
        {hosts.length === 0 ? (
          <p className="px-5 pb-4 text-[13px] text-foreground-light md:px-6">No hosts are routed to this service yet.</p>
        ) : (
          <ul className="divide-y border-t-0" aria-label="Routed hosts">
            {hosts.map(({ host, custom: isCustom }) => {
              const url = hostUrl(service.url, host)
              return (
                <li key={host} className="flex items-center gap-3 px-5 py-2.5 md:px-6">
                  <Globe className="size-4 shrink-0 text-foreground-lighter" aria-hidden="true" />
                  <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1">
                    {url ? (
                      <a
                        href={url}
                        target="_blank"
                        rel="noreferrer"
                        className="min-w-0 truncate font-mono text-[13px] text-foreground underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        {host}
                        <span className="sr-only"> (opens in a new tab)</span>
                      </a>
                    ) : (
                      <span className="min-w-0 truncate font-mono text-[13px] text-foreground">{host}</span>
                    )}
                    {isCustom ? <Badge variant="outline">Custom</Badge> : <Badge>Default</Badge>}
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    {url && (
                      <Hint label="Open">
                        <Button asChild size="icon-tiny" variant="ghost" aria-label={`Open ${host}`}>
                          <a href={url} target="_blank" rel="noreferrer">
                            <ExternalLink />
                          </a>
                        </Button>
                      </Hint>
                    )}
                    <CopyButton value={url ?? host} what={host} variant="ghost" />
                    {isCustom && (
                      <Hint label="Remove">
                        <Button
                          size="icon-tiny"
                          variant="ghost"
                          icon={<Trash2 />}
                          aria-label={`Remove ${host}`}
                          onClick={() => setToRemove(host)}
                        />
                      </Hint>
                    )}
                  </div>
                </li>
              )
            })}
          </ul>
        )}
      </FormCard>

      <ConfirmDialog
        open={toRemove !== null}
        onOpenChange={(o) => !o && setToRemove(null)}
        title={`Remove ${toRemove ?? ''}?`}
        description={<p>The proxy stops routing this domain to {name} right away. You can add it again later.</p>}
        confirmLabel="Remove domain"
        onConfirm={() => {
          const d = toRemove
          if (!d) return
          return remove.mutateAsync(d).then(() => {
            toast.success(`Removed ${d}`)
          })
        }}
      />
    </PageSection>
  )
}
