import * as React from 'react'
import { Link } from 'react-router'
import { AlertTriangle, Download, ExternalLink, FileJson, Info, Webhook } from 'lucide-react'
import { toast } from 'sonner'

import { CodeBlock, CopyField } from '@/components/patterns/Copy'
import { API_DOCS_URL, OPENAPI_URL } from '@/components/shell/nav'
import { Callout, ErrorState } from '@/components/patterns/EmptyState'
import { FormCard, FormRow } from '@/components/patterns/FormCard'
import { PageHeader, PageSection } from '@/components/patterns/Page'
import { StatePill, StatusDot } from '@/components/patterns/StatusBadge'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage, request } from '@/lib/api/client'
import { useLive, type LiveMode } from '@/lib/api/events'
import { useOpenApiSpec, useServerInfo } from '@/lib/api/queries'
import type { ServerInfo } from '@/lib/api/types'
import { bytes } from '@/lib/format'
import { defaultCpuText, defaultMemoryText, formatCpus } from '@/lib/resources'

import { GitAccountsSection } from './GitAccountsSection'
import { MaskedCommand, OnOffPill, RowValue, ThemePicker } from './parts'

function useOrigin(): string {
  return typeof window === 'undefined' ? '' : window.location.origin
}

/** Link to Server → Account, where API tokens are created. */
function AccountLink({ children }: { children: React.ReactNode }) {
  return (
    <Link
      to="/server?section=account"
      className="rounded-sm text-primary underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
    >
      {children}
    </Link>
  )
}

const LIVE_LABELS: Record<LiveMode, { label: string; hint: string }> = {
  live: { label: 'Live', hint: 'Changes arrive instantly over the /api/v1/events stream.' },
  connecting: { label: 'Connecting', hint: 'Opening the change feed; polling meanwhile.' },
  reconnecting: { label: 'Reconnecting', hint: 'The change feed dropped; polling until it is back.' },
  polling: { label: 'Polling', hint: 'This server has no change feed; the dashboard refreshes every few seconds.' },
  off: { label: 'Off', hint: 'Signed out.' },
}

function LiveStatus() {
  const mode = useLive((s) => s.mode)
  const { label, hint } = LIVE_LABELS[mode]
  return (
    <FormRow label="Live updates" description={hint}>
      <RowValue>
        <StatusDot
          tone={mode === 'live' ? 'success' : mode === 'off' ? 'neutral' : 'warning'}
          pulse={mode === 'live'}
        />
        {label}
      </RowValue>
    </FormRow>
  )
}

function InfoRowsSkeleton() {
  return (
    <FormCard asDiv>
      {Array.from({ length: 5 }, (_, i) => (
        <div key={i} className="grid gap-3 px-5 py-5 md:grid-cols-2 md:gap-8 md:px-6">
          <Skeleton className="h-4 w-32" />
          <Skeleton className="h-[34px] w-full" />
        </div>
      ))}
    </FormCard>
  )
}

function ServerInfoCard({ info }: { info: ServerInfo }) {
  return (
    <FormCard asDiv>
      <FormRow label="Ferry version" description="Version of the ferryd server this dashboard talks to.">
        <CopyField value={`v${info.version}`} what="version" aria-label="Ferry version" />
      </FormRow>
      <FormRow label="Docker engine" description="Builds and runs every service and datastore.">
        {info.docker_version ? (
          <CopyField value={info.docker_version} what="Docker version" aria-label="Docker version" />
        ) : (
          <RowValue>
            <StatePill tone="destructive" label="Unreachable" />
            <span className="text-[13px] text-foreground-light">ferryd can’t reach the Docker daemon.</span>
          </RowValue>
        )}
      </FormRow>
      <FormRow
        label="Default container limits"
        description={
          <>
            Memory and CPU of services, jobs and datastores that set no limits of their own. Set with{' '}
            <code className="font-mono text-[12.5px]">--default-memory-limit</code> and{' '}
            <code className="font-mono text-[12.5px]">--default-cpu-limit</code> (0 = unlimited).
          </>
        }
      >
        <RowValue>
          <span>
            {defaultMemoryText(info)} memory · {info.default_cpu_limit > 0 ? defaultCpuText(info) : 'unlimited CPU'}
          </span>
        </RowValue>
        {Boolean(info.docker_cpus || info.docker_memory_bytes) && (
          <p className="text-[12.5px] text-foreground-lighter">
            Docker host:{' '}
            {[info.docker_cpus ? formatCpus(info.docker_cpus) : null, info.docker_memory_bytes ? `${bytes(info.docker_memory_bytes)} memory` : null]
              .filter(Boolean)
              .join(' · ')}
          </p>
        )}
      </FormRow>
      <FormRow
        label="Base domain"
        description={
          <>
            Services are served at <span className="font-mono text-[12.5px]">&lt;name&gt;.{info.base_domain}</span>.
          </>
        }
      >
        <CopyField value={info.base_domain} what="base domain" aria-label="Base domain" />
      </FormRow>
      <FormRow label="Proxy URL" description="Public entry point of the reverse proxy that routes to your services.">
        <div className="flex gap-2">
          <CopyField value={info.proxy_url} what="proxy URL" aria-label="Proxy URL" />
          <Button asChild size="icon-md" variant="default" aria-label="Open the proxy URL">
            <a href={info.proxy_url} target="_blank" rel="noreferrer">
              <ExternalLink />
            </a>
          </Button>
        </div>
      </FormRow>
      <FormRow
        label="TLS"
        description={
          info.tls_enabled ? (
            'Automatic HTTPS (Let’s Encrypt) for custom domains.'
          ) : (
            <>
              Start ferryd with <code className="font-mono text-[12.5px]">--acme-email</code> and{' '}
              <code className="font-mono text-[12.5px]">--https-addr</code> to get certificates for custom domains.
            </>
          )
        }
      >
        <RowValue>
          <OnOffPill on={info.tls_enabled} />
        </RowValue>
      </FormRow>
      <FormRow label="Dashboard URL" description="Host routed by the proxy to this dashboard and the API.">
        {info.dashboard_url ? (
          <CopyField value={info.dashboard_url} what="dashboard URL" aria-label="Dashboard URL" />
        ) : (
          <RowValue className="text-foreground-lighter">Not configured</RowValue>
        )}
      </FormRow>
      <FormRow label="GitHub webhook" description="Deploys services with auto-deploy on every push.">
        <RowValue>
          <OnOffPill on={info.github_webhook_enabled} />
          <Link
            to="/server?section=connections"
            className="rounded-sm text-[13px] text-primary underline-offset-4 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
          >
            {info.github_webhook_enabled ? 'Webhook settings' : 'How to enable'}
          </Link>
        </RowValue>
      </FormRow>
      <LiveStatus />
    </FormCard>
  )
}

export function GeneralSection() {
  const { data: info, error, isLoading, refetch, isRefetching } = useServerInfo()
  const themeLabelId = React.useId()
  return (
    <>
      <PageHeader
        title="General"
        description="Versions, routing and TLS of this Ferry server, and how the dashboard looks."
      />
      <PageSection title="Server information">
        {isLoading ? (
          <InfoRowsSkeleton />
        ) : error || !info ? (
          <ErrorState
            error={error}
            title="Could not load server information"
            onRetry={() => void refetch()}
            retrying={isRefetching}
          />
        ) : (
          <ServerInfoCard info={info} />
        )}
      </PageSection>
      <PageSection title="Appearance">
        <FormCard asDiv>
          <FormRow
            label={<span id={themeLabelId}>Theme</span>}
            description="System follows your operating system’s light or dark setting. Saved in this browser."
          >
            <ThemePicker aria-labelledby={themeLabelId} />
          </FormRow>
        </FormCard>
      </PageSection>
    </>
  )
}

export function ConnectionsSection() {
  const origin = useOrigin()
  const { data: info, isLoading } = useServerInfo()
  const hookUrl = `${origin}/hooks/github`
  return (
    <>
      <PageHeader
        title="Connections"
        description="Connect the CLI, CI jobs and your GitHub or GitLab accounts to this server."
      />
      <PageSection title="Command line">
        <FormCard asDiv>
          <FormRow
            layout="vertical"
            label="Log in with the CLI"
            description="Opens this dashboard to approve the terminal. The terminal then gets an API token of its own, saved in ~/.config/ferry/config.json; revoke it under Account."
          >
            <CodeBlock code={`ferry login --server ${origin}`} prompt />
          </FormRow>
          <FormRow
            layout="vertical"
            label="Environment variables"
            description={
              <>
                For CI and scripts, where nobody can approve a login: create an API token under{' '}
                <AccountLink>Account</AccountLink> and export it. These override the saved config.
              </>
            }
          >
            <MaskedCommand
              display={`export FERRY_SERVER=${origin}\nexport FERRY_TOKEN=<token>`}
              value={`export FERRY_SERVER=${origin}\nexport FERRY_TOKEN=<token>`}
              what="environment variables"
            />
          </FormRow>
          <FormRow label="Server URL" description="What the CLI and API clients connect to.">
            <CopyField value={origin} what="server URL" aria-label="Server URL" />
          </FormRow>
        </FormCard>
      </PageSection>

      <GitAccountsSection />

      <PageSection
        title="GitHub webhook"
        description="Push to a branch and every service tracking it with auto-deploy is deployed."
      >
        {!isLoading && info && !info.github_webhook_enabled && (
          <Callout tone="info" icon={<Info />} title="GitHub webhooks are off">
            Start ferryd with <code className="font-mono text-[12.5px]">--github-webhook-secret &lt;secret&gt;</code>{' '}
            (or <code className="font-mono text-[12.5px]">FERRY_GITHUB_WEBHOOK_SECRET</code>), then add the webhook
            below to your repositories.
          </Callout>
        )}
        <FormCard asDiv>
          <FormRow label="Status">
            <RowValue>
              {isLoading ? <Skeleton className="h-5 w-20" /> : <OnOffPill on={Boolean(info?.github_webhook_enabled)} />}
            </RowValue>
          </FormRow>
          <FormRow
            label="Payload URL"
            description="In GitHub: Settings → Webhooks → Add webhook. It must be reachable from GitHub."
          >
            <CopyField value={hookUrl} what="webhook URL" aria-label="Payload URL" />
          </FormRow>
          <FormRow label="Content type">
            <CopyField value="application/json" what="content type" aria-label="Content type" />
          </FormRow>
          <FormRow
            label="Secret"
            description="The value ferryd was started with. It verifies the X-Hub-Signature-256 header."
          >
            <RowValue className="text-[13px] text-foreground-light">
              <span className="font-mono">--github-webhook-secret</span>
            </RowValue>
          </FormRow>
          <FormRow label="Events">
            <RowValue className="gap-1.5 text-[13px] text-foreground-light">
              <Webhook className="size-4 text-foreground-lighter" aria-hidden="true" /> Just the{' '}
              <code className="font-mono">push</code> event
            </RowValue>
          </FormRow>
        </FormCard>
      </PageSection>

      <PageSection title="Deploy hooks">
        <FormCard asDiv>
          <FormRow
            label="Per-service deploy hooks"
            description="Each service has a secret URL that triggers a deploy with a plain POST, no token needed. Find it in the service’s Settings."
          >
            <RowValue>
              <Button asChild size="sm">
                <Link to="/services">Open services</Link>
              </Button>
            </RowValue>
          </FormRow>
        </FormCard>
      </PageSection>
    </>
  )
}

async function downloadSpec() {
  try {
    const doc = await request<unknown>(OPENAPI_URL)
    const blob = new Blob([JSON.stringify(doc, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = 'ferry-openapi.json'
    document.body.appendChild(a)
    a.click()
    a.remove()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  } catch (e) {
    toast.error('Could not download the OpenAPI document', { description: errorMessage(e) })
  }
}

function ApiReferenceCard() {
  const origin = useOrigin()
  const spec = useOpenApiSpec()
  const missing = spec.missing
  return (
    <>
      {missing && (
        <Callout tone="warning" icon={<AlertTriangle />} title="This server doesn’t publish API docs yet">
          Update ferryd to a version that serves {API_DOCS_URL} and {OPENAPI_URL}. The endpoints are also listed in
          DESIGN.md §10.
        </Callout>
      )}
      <FormCard asDiv>
        <FormRow
          label="Interactive docs"
          description="Swagger UI: browse every endpoint and try requests with an API token."
        >
          <RowValue>
            {missing ? (
              <Button iconRight={<ExternalLink />} disabled>
                Open API docs
              </Button>
            ) : (
              <Button asChild variant="primary" iconRight={<ExternalLink />}>
                <a href={API_DOCS_URL} target="_blank" rel="noreferrer">
                  Open API docs
                </a>
              </Button>
            )}
          </RowValue>
        </FormRow>
        <FormRow
          label="OpenAPI specification"
          description={
            spec.isLoading ? (
              'Checking…'
            ) : spec.data ? (
              <span className="flex flex-wrap items-center gap-1.5">
                <Badge font="mono" shape="square" case="normal">
                  OpenAPI {spec.data.openapi ?? '3'}
                </Badge>
                <span>
                  {spec.data.paths} paths{spec.data.version ? ` · API v${spec.data.version}` : ''}
                </span>
              </span>
            ) : missing ? (
              'Not available on this server.'
            ) : (
              errorMessage(spec.error)
            )
          }
        >
          <div className="flex flex-col gap-2">
            <CopyField value={`${origin}${OPENAPI_URL}`} what="OpenAPI URL" aria-label="OpenAPI document URL" />
            <div>
              <Button size="tiny" icon={<Download />} disabled={!spec.data} onClick={() => void downloadSpec()}>
                Download openapi.json
              </Button>
            </div>
          </div>
        </FormRow>
      </FormCard>
    </>
  )
}

export function ApiSection() {
  const origin = useOrigin()
  return (
    <>
      <PageHeader title="API" description="Everything the dashboard and the CLI do goes through this HTTP API." />
      <PageSection title="REST API">
        <FormCard asDiv>
          <FormRow label="Base URL" description="JSON over HTTP. Errors come back as {error: {code, message}}.">
            <CopyField value={`${origin}/api/v1`} what="API base URL" aria-label="API base URL" />
          </FormRow>
          <FormRow
            layout="vertical"
            label="Authentication"
            description={
              <>
                Send an API token as a bearer token on every request. Create one under{' '}
                <AccountLink>Account</AccountLink>.
              </>
            }
          >
            <CodeBlock code="Authorization: Bearer <token>" />
          </FormRow>
          <FormRow layout="vertical" label="Example" description="List the services on this server.">
            <CodeBlock code={`curl -H "Authorization: Bearer <token>" ${origin}/api/v1/services`} prompt />
          </FormRow>
        </FormCard>
      </PageSection>
      <PageSection
        title={
          <span className="inline-flex items-center gap-2">
            <FileJson className="size-5 text-foreground-lighter" aria-hidden="true" /> Reference
          </span>
        }
      >
        <ApiReferenceCard />
      </PageSection>
      <PageSection
        title="Server-sent events"
        description="Read them with fetch(), so the token stays in the Authorization header."
      >
        <FormCard asDiv>
          <FormRow label="Change feed" description="One event per created, updated or deleted resource.">
            <CopyField value={`${origin}/api/v1/events`} what="change feed URL" aria-label="Change feed URL" />
          </FormRow>
          <FormRow label="Log streams" description="Deploy, job and runtime logs; add ?follow=true to keep streaming.">
            <CopyField
              value={`${origin}/api/v1/services/{id}/logs?follow=true`}
              what="log stream URL"
              aria-label="Log stream URL"
            />
          </FormRow>
        </FormCard>
      </PageSection>
    </>
  )
}
