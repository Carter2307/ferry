/**
 * The server's domains (services are served at `<service>.<domain>`): what
 * can be connected, how a domain, its DNS records and a certificate are
 * described.
 */

import type { StatusTone } from '@/components/patterns/status-tones'
import type {
  CertificateView,
  CheckOutcome,
  DnsRecord,
  DomainCheckKind,
  DomainView,
  ServerInfo,
} from '@/lib/api/types'
import { relativeTime } from '@/lib/format'

/**
 * The domain service URLs are shown with. A server that predates domains has
 * one domain, the one it was started with.
 */
export function defaultDomain(info: Pick<ServerInfo, 'base_domain' | 'default_domain'>): string {
  return info.default_domain || info.base_domain
}

/** How a state reads in a pill. */
export interface StateLook {
  label: string
  tone: StatusTone
  /** In progress: something will change it without the reader doing anything. */
  pulse: boolean
}

/**
 * `ferry_core::domains::connectable_name` — the domain as the server keeps
 * it (lowercase, no trailing dot), or why it can't be connected.
 */
export function checkConnectableDomain(input: string): { domain: string; error: string | null } {
  const d = input.trim().replace(/\.+$/, '').toLowerCase()
  if (!d) return { domain: d, error: 'Enter a domain.' }
  if (d.startsWith('*.')) {
    const rest = d.slice(2)
    return { domain: d, error: `Enter the domain without “*.”: services are served at <service>.${rest}.` }
  }
  if (d.includes('://') || d.includes('/')) {
    return { domain: d, error: 'Enter the domain alone, like example.com.' }
  }
  const tld = d.split('.').pop() ?? ''
  if (d.includes(':') || /^\d+$/.test(tld)) {
    return { domain: d, error: 'Enter a domain name, not an IP address.' }
  }
  const labelOk = (l: string) =>
    l.length > 0 && l.length <= 63 && !l.startsWith('-') && !l.endsWith('-') && /^[a-z0-9-]+$/.test(l)
  if (d.length > 253 || !d.includes('.') || !d.split('.').every(labelOk)) {
    return { domain: d, error: `“${d}” is not a domain name (e.g. example.com or apps.example.com).` }
  }
  return { domain: d, error: null }
}

/** How a domain is doing. */
export function domainState(d: Pick<DomainView, 'status' | 'local'>): StateLook {
  if (d.local) return { label: 'Local', tone: 'neutral', pulse: false }
  switch (d.status) {
    case 'active':
      return { label: 'Active', tone: 'success', pulse: false }
    case 'pending':
      return { label: 'Waiting for DNS', tone: 'warning', pulse: true }
    case 'misconfigured':
      return { label: 'Misconfigured', tone: 'destructive', pulse: false }
  }
}

/** Whether the DNS of the domain is something to look at: its names don't reach the server. */
export function needsDns(d: Pick<DomainView, 'status' | 'local'>): boolean {
  return !d.local && d.status !== 'active'
}

/** The name a record answers for, in full: `*.example.com`, `example.com`. */
export function recordFullName(record: Pick<DnsRecord, 'name'>, domain: string): string {
  return record.name === '@' ? domain : `${record.name}.${domain}`
}

/** What a record is for, in a few words. */
export function recordPurpose(record: Pick<DnsRecord, 'name' | 'required'>, domain: string): string {
  if (record.name === '@') return `${domain} itself. Optional: services don’t need it.`
  return record.required ? 'Every service. Required.' : 'Every service, over IPv6. Optional.'
}

/**
 * The names of the records when the DNS zone is the parent of `domain`
 * (`apps.example.com` in the zone `example.com`: `*.apps` and `apps`).
 * `null` for a name that has no parent to be a zone (`example.com`).
 */
export function namesInParentZone(domain: string): { zone: string; wildcard: string; apex: string } | null {
  const labels = domain.split('.')
  if (labels.length < 3) return null
  const [first, ...rest] = labels
  if (!first) return null
  return { zone: rest.join('.'), wildcard: `*.${first}`, apex: first }
}

export const CHECK_KIND_LABELS: Record<DomainCheckKind, string> = {
  dns: 'DNS',
  http: 'HTTP',
}

export const CHECK_OUTCOME_TONE: Record<CheckOutcome, StatusTone> = {
  passed: 'success',
  warning: 'warning',
  failed: 'destructive',
  skipped: 'neutral',
}

/** A certificate the server is about to ask for, or is asking for. */
export function certificateOnItsWay(c: Pick<CertificateView, 'state'>): boolean {
  return c.state === 'pending' || c.state === 'issuing'
}

/** Where the certificate of a hostname is, and what to know about it. */
export function certificateLook(
  c: Pick<CertificateView, 'state' | 'expires_at' | 'error' | 'retry_at'>,
  now: number = Date.now(),
): StateLook & { detail: string } {
  switch (c.state) {
    case 'issued':
      return {
        label: 'Issued',
        tone: 'success',
        pulse: false,
        detail: c.expires_at ? `Expires ${relativeTime(c.expires_at, now)}; renewed before that.` : '',
      }
    case 'issuing':
      return { label: 'Issuing', tone: 'info', pulse: true, detail: 'Being requested from the certificate authority.' }
    case 'pending':
      return { label: 'Pending', tone: 'info', pulse: true, detail: 'Requested within seconds.' }
    case 'failed': {
      const retry = c.retry_at ? ` Next attempt ${relativeTime(c.retry_at, now)}.` : ''
      const error = (c.error ?? 'The certificate could not be issued').replace(/\.*$/, '.')
      return {
        label: 'Failed',
        tone: 'destructive',
        pulse: false,
        detail: `${error.charAt(0).toUpperCase()}${error.slice(1)}${retry}`,
      }
    }
    case 'local':
      return { label: 'Local', tone: 'neutral', pulse: false, detail: 'A local name: served over HTTP, no certificate.' }
    case 'disabled':
      return { label: 'Off', tone: 'neutral', pulse: false, detail: 'HTTPS is off on this server.' }
  }
}
