import * as React from 'react'
import { Link } from 'react-router'
import {
  AlertTriangle,
  Check,
  ChevronRight,
  Globe,
  Info,
  Lock,
  Minus,
  Plus,
  RefreshCw,
  Star,
  Unplug,
  X,
  type LucideIcon,
} from 'lucide-react'
import { toast } from 'sonner'

import { ConfirmDialog } from '@/components/patterns/ConfirmDialog'
import { CopyButton } from '@/components/patterns/Copy'
import { Callout, EmptyState, ErrorState } from '@/components/patterns/EmptyState'
import { PageHeader, PageSection } from '@/components/patterns/Page'
import { StatePill } from '@/components/patterns/StatusBadge'
import type { StatusTone } from '@/components/patterns/status-tones'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
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
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { errorMessage } from '@/lib/api/client'
import {
  useCertificates,
  useConnectDomain,
  useDisconnectDomain,
  useServerDomains,
  useServerInfo,
  useSetDefaultDomain,
  useVerifyDomain,
} from '@/lib/api/queries'
import type { CertificateView, CheckOutcome, DnsRecord, DomainCheck, DomainView } from '@/lib/api/types'
import {
  CHECK_KIND_LABELS,
  CHECK_OUTCOME_TONE,
  certificateLook,
  checkConnectableDomain,
  domainState,
  namesInParentZone,
  needsDns,
  recordFullName,
  recordPurpose,
} from '@/lib/domains'
import { cn } from '@/lib/utils'
import { Field, FormError } from '@/pages/login/AuthLayout'
import { fieldAria } from '@/pages/login/forms'
import { useNow } from '@/pages/service/config/hooks'
import { since } from '@/pages/services/lib'

const code = 'font-mono text-[12.5px]'

/** `https://<service>.example.com`, in the monospace face. */
function Pattern({ children }: { children: string }) {
  return <span className={cn(code, 'text-foreground-light [overflow-wrap:anywhere]')}>{children}</span>
}

// ---------------------------------------------------------------------------
// DNS records and what a verification found

/** The DNS records that point a domain at this server, each value with its copy button. */
export function DnsRecords({ domain, records }: { domain: string; records: DnsRecord[] }) {
  const parent = namesInParentZone(domain)
  const unknown = records.some((r) => r.value === null)
  return (
    <div className="flex flex-col gap-2.5">
      <Table aria-label={`DNS records of ${domain}`} containerClassName="shadow-none">
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead className="w-16">Type</TableHead>
            <TableHead>Name</TableHead>
            <TableHead>Value</TableHead>
            <TableHead className="hidden md:table-cell">For</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {records.map((r) => (
            <TableRow key={`${r.type} ${r.name} ${r.value ?? ''}`} className="hover:bg-transparent">
              <TableCell>
                <Badge font="mono" shape="square" case="normal">
                  {r.type}
                </Badge>
              </TableCell>
              <TableCell>
                <span className="flex items-center gap-1">
                  <span className="font-mono text-[13px]">{r.name}</span>
                  <CopyButton value={r.name} what={`the name ${r.name}`} variant="ghost" />
                </span>
                <span className="block font-mono text-[11.5px] text-foreground-lighter">{recordFullName(r, domain)}</span>
                {/* phones have no room for the "For" column */}
                <span className="block text-[11.5px] text-foreground-lighter md:hidden">
                  {r.required ? 'Required' : 'Optional'}
                </span>
              </TableCell>
              <TableCell>
                {r.value ? (
                  <span className="flex items-center gap-1">
                    <span className="font-mono text-[13px]">{r.value}</span>
                    <CopyButton value={r.value} what={`the address ${r.value}`} variant="ghost" />
                  </span>
                ) : (
                  <span className="text-[13px] text-foreground-light">The public IP address of this server</span>
                )}
              </TableCell>
              <TableCell className="hidden text-[12.5px] whitespace-normal text-foreground-lighter md:table-cell">
                {recordPurpose(r, domain)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <p className="text-[12.5px] leading-relaxed text-foreground-lighter">
        <span className={code}>*</span> is a wildcard: one record that answers for every name under {domain}, so a new
        service needs no new record.
        {parent && (
          <>
            {' '}
            If {domain} lives in the DNS zone of {parent.zone}, name the records{' '}
            <span className={code}>{parent.wildcard}</span> and <span className={code}>{parent.apex}</span> there.
          </>
        )}
      </p>
      {unknown && (
        <Callout tone="warning" icon={<AlertTriangle />}>
          Ferry could not find the public address of this server. Point the record at the address you reach the server
          at, and start ferryd with <code className={cn(code, 'whitespace-nowrap')}>--public-ip &lt;address&gt;</code> so
          it can tell when the DNS is right.
        </Callout>
      )}
    </div>
  )
}

const CHECK_ICONS: Record<CheckOutcome, LucideIcon> = {
  passed: Check,
  warning: AlertTriangle,
  failed: X,
  skipped: Minus,
}

const INK: Record<StatusTone, string> = {
  success: 'text-success',
  warning: 'text-warning',
  destructive: 'text-destructive',
  info: 'text-info',
  neutral: 'text-foreground-lighter',
}

/** What the last verification found, one line per check. */
export function Checks({ checks }: { checks: DomainCheck[] }) {
  if (checks.length === 0) return null
  return (
    <ul className="flex flex-col gap-1.5" aria-label="Last verification">
      {checks.map((c) => {
        const Icon = CHECK_ICONS[c.outcome]
        return (
          <li key={c.kind} className="flex items-start gap-2 text-[13px] leading-relaxed">
            <Icon className={cn('mt-[3px] size-3.5 shrink-0', INK[CHECK_OUTCOME_TONE[c.outcome]])} aria-hidden="true" />
            <span className="w-9 shrink-0 pt-px font-mono text-[11.5px] tracking-[0.06em] text-foreground-lighter uppercase">
              {CHECK_KIND_LABELS[c.kind]}
            </span>
            <span className="min-w-0 break-words text-foreground-light">
              <span className="sr-only">{c.outcome}: </span>
              {c.message}
            </span>
          </li>
        )
      })}
    </ul>
  )
}

/** The records to create for a domain, then what the server saw the last time it looked. */
function DomainDns({ domain, now }: { domain: DomainView; now: number }) {
  return (
    <div className="flex flex-col gap-4">
      <DnsRecords domain={domain.name} records={domain.records} />
      {domain.checks.length > 0 && (
        <div className="flex flex-col gap-2">
          <p className="text-[12.5px] text-foreground-lighter">
            Last verification
            {domain.checked_at && <span title={domain.checked_at}> · {since(domain.checked_at, now)}</span>}
          </p>
          <Checks checks={domain.checks} />
        </div>
      )}
    </div>
  )
}

/** The check that says why a domain doesn't reach the server. */
function firstProblem(domain: DomainView): string | undefined {
  const by = (outcome: CheckOutcome) => domain.checks.find((c) => c.outcome === outcome)?.message
  return by('failed') ?? by('warning')
}

/** Tell what a verification asked for by hand found. */
function reportVerification(domain: DomainView): void {
  if (domain.local || domain.status === 'active') {
    toast.success(`${domain.name} reaches this server`, { description: `Services are served at ${domain.url_pattern}` })
  } else {
    toast.warning(`${domain.name} doesn’t reach this server`, { description: firstProblem(domain) })
  }
}

// ---------------------------------------------------------------------------
// connect

/**
 * "Connect a domain" modal: the name, then — until its names reach the
 * server — the DNS record to create and what the server sees of it.
 */
function ConnectDomainDialog({
  open,
  onOpenChange,
  domains,
  now,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  domains: DomainView[]
  now: number
}) {
  const connect = useConnectDomain()
  const verify = useVerifyDomain()
  const [input, setInput] = React.useState('')
  const [touched, setTouched] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const [created, setCreated] = React.useState<DomainView | null>(null)
  const inputId = React.useId()

  // The list is kept up to date by the change feed: follow the domain there.
  const domain = created ? (domains.find((d) => d.id === created.id) ?? created) : null

  const close = () => {
    onOpenChange(false)
    setInput('')
    setTouched(false)
    setError(null)
    setCreated(null)
    connect.reset()
  }

  const check = checkConnectableDomain(input)
  const existing = check.error ? undefined : domains.find((d) => d.name === check.domain)
  const inputError =
    check.error ??
    (existing
      ? existing.source === 'config'
        ? `${existing.name} is the base domain of this server.`
        : `${existing.name} is already connected.`
      : null)
  const hint = inputError ? (
    'A domain or a subdomain you own: example.com, apps.example.com.'
  ) : (
    <>
      Services will be served at <span className={code}>&lt;service&gt;.{check.domain}</span>
    </>
  )

  const submit = async () => {
    setTouched(true)
    if (inputError || connect.isPending) return
    setError(null)
    try {
      setCreated(await connect.mutateAsync(check.domain))
    } catch (e) {
      setError(errorMessage(e))
    }
  }

  const reached = domain !== null && !needsDns(domain)

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (connect.isPending) return
        if (o) onOpenChange(true)
        else close()
      }}
    >
      <DialogContent size={domain && !reached ? 'xl' : 'lg'}>
        {domain === null ? (
          <form
            noValidate
            className="flex min-h-0 flex-col"
            onSubmit={(e) => {
              e.preventDefault()
              void submit()
            }}
          >
            <DialogHeader>
              <DialogTitle>Connect a domain</DialogTitle>
              <DialogDescription>
                Once its DNS points at this server, every service gets a hostname under it, on top of the ones it has.
              </DialogDescription>
            </DialogHeader>
            <DialogBody className="gap-5">
              <Field id={inputId} label="Domain" hint={hint} error={touched ? inputError : null}>
                <Input
                  id={inputId}
                  mono
                  autoFocus
                  autoComplete="off"
                  autoCapitalize="none"
                  spellCheck={false}
                  placeholder="example.com"
                  value={input}
                  onChange={(e) => {
                    setInput(e.target.value)
                    setError(null)
                  }}
                  {...fieldAria(inputId, true, touched ? inputError : null)}
                />
              </Field>
              {error && <FormError>{error}</FormError>}
            </DialogBody>
            <DialogFooter>
              <Button disabled={connect.isPending} onClick={close}>
                Cancel
              </Button>
              <Button type="submit" variant="primary" loading={connect.isPending}>
                Connect domain
              </Button>
            </DialogFooter>
          </form>
        ) : reached ? (
          <>
            <DialogHeader>
              <DialogTitle>{domain.name} is connected</DialogTitle>
              <DialogDescription>
                {domain.local ? 'A local name: there is nothing to verify.' : 'Its names reach this server.'}
              </DialogDescription>
            </DialogHeader>
            <DialogBody>
              <Callout tone="success" icon={<Check />}>
                <p>
                  Services are served at <Pattern>{domain.url_pattern}</Pattern>
                </p>
                {domain.is_default && <p>It is the default domain: service URLs are shown with it.</p>}
              </Callout>
            </DialogBody>
            <DialogFooter>
              <Button variant="primary" onClick={close}>
                Done
              </Button>
            </DialogFooter>
          </>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>Point {domain.name} at this server</DialogTitle>
              <DialogDescription>
                Create the required record where the DNS of {domain.name} is managed: your registrar or your DNS
                provider. Ferry checks every few seconds and serves the domain as soon as it answers.
              </DialogDescription>
            </DialogHeader>
            <DialogBody className="gap-5">
              <DomainDns domain={domain} now={now} />
            </DialogBody>
            <DialogFooter className="sm:justify-between">
              <span className="flex items-center gap-2 text-[12.5px] text-foreground-lighter">
                <StatePill {...domainState(domain)} />
                You can close this window: the check goes on.
              </span>
              <span className="flex flex-col-reverse gap-2 sm:flex-row sm:items-center">
                <Button
                  icon={<RefreshCw />}
                  loading={verify.isPending}
                  onClick={() =>
                    verify.mutate(domain.id, {
                      // Reached: this window says so itself.
                      onSuccess: (d) => needsDns(d) && reportVerification(d),
                      onError: (e) => toast.error(`Could not verify ${domain.name}`, { description: errorMessage(e) }),
                    })
                  }
                >
                  Verify now
                </Button>
                <Button variant="primary" onClick={close}>
                  Done
                </Button>
              </span>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}

// ---------------------------------------------------------------------------
// the list

/** Where services are found under a domain, or what keeps them from it. */
function Summary({ domain }: { domain: DomainView }) {
  const at = <Pattern>{domain.url_pattern}</Pattern>
  if (domain.local) return <>Services are served at {at}. A local name: nothing to verify.</>
  if (domain.status === 'active') return <>Services are served at {at}</>
  if (domain.status === 'misconfigured') {
    return <>Its DNS no longer points at this server. Services are still served at {at}</>
  }
  if (domain.served) return <>Services are served at {at}, but its DNS doesn’t point at this server yet.</>
  return <>Services will be served at {at} once its DNS points at this server.</>
}

/** Where the domain comes from and when it was last looked at. */
function History({ domain, now }: { domain: DomainView; now: number }) {
  const parts: React.ReactNode[] = [
    domain.source === 'config' ? (
      <>
        Set with <span className={code}>--base-domain</span>
      </>
    ) : (
      <span title={domain.created_at}>Connected {since(domain.created_at, now)}</span>
    ),
  ]
  // What an active domain was last seen doing is what its last check saw.
  if (needsDns(domain) && domain.verified_at) {
    parts.push(<span title={domain.verified_at}>last reached {since(domain.verified_at, now)}</span>)
  }
  if (!domain.local && domain.checked_at) {
    parts.push(<span title={domain.checked_at}>checked {since(domain.checked_at, now)}</span>)
  }
  return (
    <>
      {parts.map((part, i) => (
        <React.Fragment key={i}>
          {i > 0 && ' · '}
          {part}
        </React.Fragment>
      ))}
    </>
  )
}

export function DomainRow({
  domain,
  now,
  onDisconnect,
}: {
  domain: DomainView
  now: number
  onDisconnect: () => void
}) {
  const verify = useVerifyDomain()
  const makeDefault = useSetDefaultDomain()
  // Open while there is something to do about the DNS, unless chosen otherwise.
  const [chosen, setChosen] = React.useState<boolean | null>(null)
  const open = chosen ?? needsDns(domain)

  return (
    <li className="flex flex-col gap-3 px-5 py-4 md:px-6">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <span
          aria-hidden="true"
          className="flex size-9 shrink-0 items-center justify-center rounded-md border bg-surface-100 text-foreground-light dark:bg-surface-200"
        >
          <Globe className="size-[18px]" />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm">
            <span className="truncate font-medium text-foreground">{domain.name}</span>
            <StatePill {...domainState(domain)} />
            {domain.is_default && <Badge variant="outline">Default</Badge>}
          </p>
          <p className="text-[12.5px] text-foreground-lighter">
            <Summary domain={domain} />
          </p>
          <p className="text-[12.5px] text-foreground-lighter">
            <History domain={domain} now={now} />
          </p>
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-2">
          {!domain.local && (
            <Button
              size="tiny"
              icon={<RefreshCw />}
              loading={verify.isPending}
              onClick={() =>
                verify.mutate(domain.id, {
                  onSuccess: reportVerification,
                  onError: (e) => toast.error(`Could not verify ${domain.name}`, { description: errorMessage(e) }),
                })
              }
            >
              Verify now
            </Button>
          )}
          {domain.served && !domain.is_default && (
            <Button
              size="tiny"
              icon={<Star />}
              loading={makeDefault.isPending}
              onClick={() =>
                makeDefault.mutate(domain.id, {
                  onSuccess: (d) =>
                    toast.success(`${d.name} is the default domain`, {
                      description:
                        'Service URLs are shown with it. Running services read their new FERRY_EXTERNAL_URL at their next deploy or restart.',
                    }),
                  onError: (e) =>
                    toast.error(`Could not make ${domain.name} the default`, { description: errorMessage(e) }),
                })
              }
            >
              Make default
            </Button>
          )}
          {domain.source === 'connected' && (
            <Button size="tiny" variant="danger" icon={<Unplug />} onClick={onDisconnect}>
              Disconnect
            </Button>
          )}
        </div>
      </div>
      {!domain.local && (
        <Collapsible open={open} onOpenChange={setChosen} className="sm:pl-12">
          <CollapsibleTrigger className="group inline-flex cursor-pointer items-center gap-1 rounded-sm text-[12.5px] text-foreground-light outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring">
            <ChevronRight className="size-3.5 transition-transform group-data-[state=open]:rotate-90" aria-hidden="true" />
            DNS records{domain.checks.length > 0 && ' and last verification'}
          </CollapsibleTrigger>
          <CollapsibleContent className="pt-3">
            <DomainDns domain={domain} now={now} />
          </CollapsibleContent>
        </Collapsible>
      )}
    </li>
  )
}

function DomainList({ now }: { now: number }) {
  const domains = useServerDomains()
  const disconnect = useDisconnectDomain()
  const [connecting, setConnecting] = React.useState(false)
  const [leaving, setLeaving] = React.useState<DomainView | null>(null)
  const list = domains.data ?? []
  const fallback = list.find((d) => d.source === 'config')
  // A server that only answers on local names has nothing public to show yet.
  const onlyLocal = list.length > 0 && list.every((d) => d.local)

  return (
    <>
      <PageHeader
        title="Domains"
        description={
          <>
            Every service is served at <span className="font-mono text-[13px]">&lt;service&gt;.&lt;domain&gt;</span>{' '}
            under each domain of this server. The default one is the address services are shown with.
          </>
        }
        actions={
          domains.data ? (
            <Button size="sm" variant="primary" icon={<Plus />} onClick={() => setConnecting(true)}>
              Connect a domain
            </Button>
          ) : undefined
        }
      />
      <PageSection id="domains" aria-label="Domains of this server">
        {domains.isPending ? (
          <Skeleton className="h-[92px] w-full" />
        ) : domains.isError && !domains.data ? (
          <ErrorState
            error={domains.error}
            title="Could not load the domains"
            onRetry={() => void domains.refetch()}
            retrying={domains.isRefetching}
          />
        ) : (
          <>
            {onlyLocal && (
              <Callout tone="info" icon={<Info />} title="This server only answers on local names">
                They work on this machine and nowhere else. Connect a domain you own to give every service a public
                address like <span className={code}>&lt;service&gt;.example.com</span>: it takes one DNS record.
              </Callout>
            )}
            <Card>
              <ul className="divide-y" aria-label="Domains">
                {list.map((d) => (
                  <DomainRow key={d.id} domain={d} now={now} onDisconnect={() => setLeaving(d)} />
                ))}
              </ul>
            </Card>
          </>
        )}

        <ConnectDomainDialog open={connecting} onOpenChange={setConnecting} domains={list} now={now} />

        <ConfirmDialog
          open={leaving !== null}
          onOpenChange={(o) => !o && setLeaving(null)}
          title={`Disconnect ${leaving?.name ?? ''}?`}
          description={
            <p>
              {leaving?.served
                ? `Services stop being served under ${leaving.name} right away.`
                : `Ferry stops waiting for the DNS of ${leaving?.name ?? ''}.`}{' '}
              {leaving?.local ? '' : 'A DNS record you created for it stays at your DNS provider.'}
            </p>
          }
          confirmLabel="Disconnect"
          onConfirm={() => {
            const d = leaving
            if (!d) return
            return disconnect.mutateAsync(d.id).then(() => {
              toast.success(`${d.name} disconnected`)
            })
          }}
        >
          {leaving?.is_default && (
            <Callout tone="warning" icon={<AlertTriangle />}>
              It is the default domain: service URLs go back to {fallback?.name ?? 'the base domain'}.
            </Callout>
          )}
        </ConfirmDialog>
      </PageSection>
    </>
  )
}

// ---------------------------------------------------------------------------
// certificates

export function CertificatesTable({ certificates, now }: { certificates: CertificateView[]; now: number }) {
  return (
    <Table aria-label="Certificates">
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead className="w-[36%]">Hostname</TableHead>
          <TableHead className="w-[18%]">Routed to</TableHead>
          <TableHead>Certificate</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {certificates.map((c) => {
          const look = certificateLook(c, now)
          return (
            <TableRow key={c.host} className="hover:bg-transparent">
              <TableCell className="font-mono text-[13px] whitespace-normal [overflow-wrap:anywhere]">{c.host}</TableCell>
              <TableCell>
                {c.service ? (
                  <Link
                    to={`/services/${encodeURIComponent(c.service)}`}
                    className="rounded-sm text-[13px] text-foreground-light underline-offset-4 outline-none hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    {c.service}
                  </Link>
                ) : (
                  <span className="text-[13px] text-foreground-lighter">Dashboard</span>
                )}
              </TableCell>
              <TableCell className="whitespace-normal">
                <span className="flex flex-col items-start gap-1 py-1">
                  <StatePill tone={look.tone} label={look.label} pulse={look.pulse} />
                  {look.detail && (
                    <span
                      className={cn(
                        'text-[12.5px] leading-relaxed break-words',
                        c.state === 'failed' ? 'text-foreground-light' : 'text-foreground-lighter',
                      )}
                      title={c.expires_at ?? c.retry_at ?? undefined}
                    >
                      {look.detail}
                    </span>
                  )}
                </span>
              </TableCell>
            </TableRow>
          )
        })}
      </TableBody>
    </Table>
  )
}

function Certificates({ now }: { now: number }) {
  const info = useServerInfo()
  const off = info.data ? !info.data.tls_enabled : undefined
  const certificates = useCertificates({ enabled: off === false })
  const list = certificates.data ?? []

  return (
    <PageSection
      id="certificates"
      title="Certificates"
      description="What lets browsers reach a hostname over HTTPS. Each public hostname gets its own, asked from Let’s Encrypt as soon as the name is routed, and renewed before it expires."
    >
      {off === undefined ? (
        info.isError ? (
          <ErrorState
            error={info.error}
            title="Could not load server information"
            onRetry={() => void info.refetch()}
            retrying={info.isRefetching}
          />
        ) : (
          <Skeleton className="h-[92px] w-full" />
        )
      ) : off ? (
        <Callout tone="neutral" icon={<Lock />} title="HTTPS is off on this server">
          Services are served over plain HTTP. Start ferryd with{' '}
          <code className={cn(code, 'whitespace-nowrap')}>--https-addr 0.0.0.0:443</code> and{' '}
          <code className={cn(code, 'whitespace-nowrap')}>--acme-email &lt;your email&gt;</code>: every public hostname
          then gets its certificate on its own.
        </Callout>
      ) : certificates.isPending ? (
        <Skeleton className="h-[92px] w-full" />
      ) : certificates.isError && !certificates.data ? (
        <ErrorState
          error={certificates.error}
          title="Could not load the certificates"
          onRetry={() => void certificates.refetch()}
          retrying={certificates.isRefetching}
        />
      ) : list.length === 0 ? (
        <EmptyState
          icon={<Lock />}
          title="No hostname is routed yet"
          description="Create a web service or a static site: its hostnames are listed here with their certificate."
        />
      ) : (
        <CertificatesTable certificates={list} now={now} />
      )}
    </PageSection>
  )
}

/**
 * Server → Domains: the domains services are served under (the one the
 * server was started with and the ones connected here), the DNS record each
 * one needs and whether it reaches the server, then the certificate of every
 * routed hostname.
 */
export function DomainsSection() {
  const now = useNow(10_000)
  return (
    <>
      <DomainList now={now} />
      <Certificates now={now} />
    </>
  )
}
