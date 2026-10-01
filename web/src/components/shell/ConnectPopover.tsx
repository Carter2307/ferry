import * as React from 'react'
import { Link } from 'react-router'
import { Plug } from 'lucide-react'

import { CodeBlock, CopyField, SecretField } from '@/components/patterns/Copy'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useService } from '@/lib/api/queries'

import { useRouteResource } from './useRouteResource'

function Section({ label, children, hint }: { label: string; children: React.ReactNode; hint?: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <MonoLabel>{label}</MonoLabel>
      {children}
      {hint && <p className="text-[12px] text-foreground-lighter">{hint}</p>}
    </div>
  )
}

/** Pill "Connect" button: CLI login command, API URL and (on a service) its URLs + deploy hook. */
export function ConnectPopover({ compact = false }: { compact?: boolean }) {
  const resource = useRouteResource()
  const serviceName = resource.kind === 'service' ? resource.name : undefined
  const { data: service } = useService(serviceName)
  const origin = typeof window !== 'undefined' ? window.location.origin : ''
  const loginCmd = `ferry login --server ${origin}`
  const hookUrl = service ? `${origin}${service.deploy_hook_path}` : null

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          shape="pill"
          size={compact ? 'icon' : 'sm'}
          icon={<Plug className="-rotate-45" />}
          aria-label={compact ? 'Connect' : undefined}
          className="text-foreground-light"
        >
          {!compact && 'Connect'}
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-[min(440px,calc(100vw-24px))] p-0 outline-none"
        tabIndex={-1}
        aria-label="Connect to this server"
        onOpenAutoFocus={(e) => {
          // focus the panel itself (not the first Copy button, which would pop its tooltip)
          e.preventDefault()
          ;(e.currentTarget as HTMLElement | null)?.focus()
        }}
      >
        <div className="border-b px-4 py-3">
          <p className="text-sm font-medium text-foreground">Connect to this server</p>
          <p className="text-[13px] text-foreground-light">Use the CLI, or the HTTP API with an API token.</p>
        </div>
        <div className="flex flex-col gap-4 px-4 py-4">
          <Section
            label="CLI login"
            hint="Opens this dashboard to approve the terminal, which then gets an API token of its own."
          >
            <CodeBlock code={loginCmd} prompt />
          </Section>
          <Section
            label="API URL"
            hint={
              <>
                Send an API token as <code className="font-mono">Authorization: Bearer &lt;token&gt;</code>. Create one
                under{' '}
                <Link
                  to="/server?section=account"
                  className="rounded-sm text-primary underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                >
                  Server → Account
                </Link>
                .
              </>
            }
          >
            <CopyField value={`${origin}/api/v1`} size="sm" what="API URL" aria-label="API URL" />
          </Section>
          {service && (
            <>
              {service.url && (
                <Section label="Service URL">
                  <CopyField value={service.url} size="sm" what="service URL" aria-label="Service URL" />
                </Section>
              )}
              <Section label="Private address" hint="Reachable from other services and jobs on the private network.">
                <CopyField
                  value={service.internal_port ? `${service.internal_host}:${service.internal_port}` : service.internal_host}
                  size="sm"
                  what="private address"
                  aria-label="Private address"
                />
              </Section>
              {hookUrl && (
                <Section label="Deploy hook" hint="POST to this secret URL to trigger a deploy (no token needed).">
                  <SecretField value={hookUrl} size="sm" what="deploy hook URL" aria-label="Deploy hook URL" />
                </Section>
              )}
            </>
          )}
        </div>
      </PopoverContent>
    </Popover>
  )
}
