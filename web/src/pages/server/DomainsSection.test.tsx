import * as React from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderToStaticMarkup } from 'react-dom/server'
import { MemoryRouter } from 'react-router'
import { describe, expect, it } from 'vitest'

import { TooltipProvider } from '@/components/ui/tooltip'
import type { CertificateView, DnsRecord, DomainView } from '@/lib/api/types'

import { CertificatesTable, Checks, DnsRecords, DomainRow } from './DomainsSection'

const NOW = Date.parse('2026-10-02T12:00:00Z')

/** Visible text of what `node` renders (tags stripped, whitespace collapsed). */
function text(node: React.ReactNode): string {
  const html = renderToStaticMarkup(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <TooltipProvider>{node}</TooltipProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  )
  return html
    .replace(/<[^>]+>/g, ' ')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&#x27;/g, "'")
    .replace(/&amp;/g, '&')
    .replace(/\s+/g, ' ')
    .trim()
}

const RECORDS: DnsRecord[] = [
  { type: 'A', name: '*', value: '203.0.113.10', required: true },
  { type: 'A', name: '@', value: '203.0.113.10', required: false },
]

function domain(patch: Partial<DomainView>): DomainView {
  return {
    id: 'dom-1',
    name: 'example.com',
    source: 'connected',
    status: 'active',
    is_default: false,
    checks: [],
    created_at: '2026-10-02T11:00:00Z',
    verified_at: '2026-10-02T11:58:00Z',
    checked_at: '2026-10-02T11:58:00Z',
    local: false,
    served: true,
    records: RECORDS,
    url_pattern: 'https://<service>.example.com',
    ...patch,
  }
}

describe('DnsRecords', () => {
  it('lists each record with the name it answers for and what it is for', () => {
    const out = text(<DnsRecords domain="example.com" records={RECORDS} />)
    expect(out).toContain('*.example.com')
    expect(out).toContain('203.0.113.10')
    expect(out).toContain('Every service. Required.')
    expect(out).toContain('example.com itself. Optional')
    expect(out).toContain('is a wildcard')
    expect(out).not.toContain('lives in the DNS zone')
    expect(out).not.toContain('--public-ip')
  })

  it('says how the records are named in the zone of a parent domain', () => {
    const out = text(<DnsRecords domain="apps.example.com" records={RECORDS} />)
    expect(out).toContain('If apps.example.com lives in the DNS zone of example.com, name the records *.apps and apps there.')
  })

  it('asks for the address when the server could not find its own', () => {
    const out = text(<DnsRecords domain="example.com" records={[{ type: 'A', name: '*', value: null, required: true }]} />)
    expect(out).toContain('The public IP address of this server')
    expect(out).toContain('--public-ip <address>')
  })
})

describe('Checks', () => {
  it('shows what each check found', () => {
    const out = text(
      <Checks
        checks={[
          { kind: 'dns', outcome: 'failed', message: 'No DNS record answers for *.example.com yet.' },
          { kind: 'http', outcome: 'skipped', message: 'Not checked: the name does not resolve.' },
        ]}
      />,
    )
    expect(out).toContain('DNS failed: No DNS record answers for *.example.com yet.')
    expect(out).toContain('HTTP skipped: Not checked: the name does not resolve.')
    expect(text(<Checks checks={[]} />)).toBe('')
  })
})

describe('DomainRow', () => {
  const row = (d: DomainView) => text(<DomainRow domain={d} now={NOW} onDisconnect={() => {}} />)

  it('shows the record to create while a domain waits for its DNS', () => {
    const out = row(
      domain({
        status: 'pending',
        served: false,
        verified_at: null,
        checked_at: '2026-10-02T11:59:50Z',
        checks: [{ kind: 'dns', outcome: 'failed', message: 'No DNS record answers for *.example.com yet.' }],
      }),
    )
    expect(out).toContain('Waiting for DNS')
    expect(out).toContain('Services will be served at https://<service>.example.com once its DNS points at this server.')
    expect(out).toContain('Connected 1h ago · checked 10s ago')
    // open without a click: there is something to do
    expect(out).toContain('203.0.113.10')
    expect(out).toContain('No DNS record answers for *.example.com yet.')
    expect(out).toContain('Verify now')
    expect(out).toContain('Disconnect')
    expect(out).not.toContain('Make default')
  })

  it('offers to make a served domain the default, and keeps its records folded', () => {
    const out = row(domain({}))
    expect(out).toContain('Active')
    expect(out).toContain('Services are served at https://<service>.example.com')
    expect(out).toContain('Make default')
    expect(out).toContain('DNS records')
    expect(out).not.toContain('203.0.113.10')
    expect(row(domain({ is_default: true }))).not.toContain('Make default')
  })

  it('says when a domain that stopped reaching the server last did', () => {
    const out = row(domain({ status: 'misconfigured', verified_at: '2026-10-02T09:00:00Z' }))
    expect(out).toContain('Misconfigured')
    expect(out).toContain('Its DNS no longer points at this server. Services are still served at')
    expect(out).toContain('last reached 3h ago · checked 2m ago')
  })

  it('neither verifies a local name nor disconnects the base domain', () => {
    const out = row(
      domain({
        name: 'localhost',
        source: 'config',
        local: true,
        is_default: true,
        records: [],
        verified_at: null,
        checked_at: null,
        url_pattern: 'http://<service>.localhost:8080',
      }),
    )
    expect(out).toContain('Local')
    expect(out).toContain('Default')
    expect(out).toContain('Set with --base-domain')
    expect(out).toContain('A local name: nothing to verify.')
    for (const absent of ['Verify now', 'Disconnect', 'Make default', 'DNS records']) {
      expect(out, absent).not.toContain(absent)
    }
  })
})

describe('CertificatesTable', () => {
  const none = { expires_at: null, error: null, retry_at: null }
  const certificates: CertificateView[] = [
    { ...none, host: 'ferry.example.com', service: null, state: 'issued', expires_at: '2026-10-22T12:00:00Z' },
    { ...none, host: 'web.example.com', service: 'web', state: 'issuing' },
    {
      ...none,
      host: 'api.example.com',
      service: 'api',
      state: 'failed',
      error: 'no A record for api.example.com',
      retry_at: '2026-10-02T12:04:50Z',
    },
    { ...none, host: 'web.localhost', service: 'web', state: 'local' },
  ]

  it('says where the certificate of each hostname is', () => {
    const out = text(<CertificatesTable certificates={certificates} now={NOW} />)
    expect(out).toContain('ferry.example.com Dashboard Issued Expires in 20d; renewed before that.')
    expect(out).toContain('web.example.com web Issuing')
    expect(out).toContain('api.example.com api Failed No A record for api.example.com. Next attempt in 4m.')
    expect(out).toContain('web.localhost web Local')
  })
})
