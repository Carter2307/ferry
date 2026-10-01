import { describe, expect, it } from 'vitest'

import {
  certificateLook,
  certificateOnItsWay,
  checkConnectableDomain,
  domainState,
  namesInParentZone,
  needsDns,
  recordFullName,
  recordPurpose,
} from './domains'

describe('checkConnectableDomain', () => {
  it('normalizes what the server would keep', () => {
    expect(checkConnectableDomain(' Example.COM. ')).toEqual({ domain: 'example.com', error: null })
    expect(checkConnectableDomain('apps.example.com').error).toBeNull()
    expect(checkConnectableDomain('dev.localhost').error).toBeNull()
  })

  it('says what to enter instead of a wildcard, a URL or an address', () => {
    expect(checkConnectableDomain('').error).toBe('Enter a domain.')
    expect(checkConnectableDomain('*.example.com').error).toContain('<service>.example.com')
    expect(checkConnectableDomain('https://example.com').error).toBe('Enter the domain alone, like example.com.')
    expect(checkConnectableDomain('example.com/app').error).toBe('Enter the domain alone, like example.com.')
    for (const ip of ['203.0.113.10', '2001:db8::1']) {
      expect(checkConnectableDomain(ip).error).toBe('Enter a domain name, not an IP address.')
    }
  })

  it('refuses what is not a domain name', () => {
    for (const bad of ['localhost', 'exa mple.com', '-a.com', 'a-.com', 'a..com', `${'a'.repeat(64)}.com`]) {
      expect(checkConnectableDomain(bad).error, bad).toMatch(/is not a domain name/)
    }
  })
})

describe('domainState', () => {
  it('describes a domain by whether its names reach the server', () => {
    expect(domainState({ status: 'active', local: false })).toEqual({ label: 'Active', tone: 'success', pulse: false })
    expect(domainState({ status: 'pending', local: false })).toMatchObject({ label: 'Waiting for DNS', pulse: true })
    expect(domainState({ status: 'misconfigured', local: false })).toMatchObject({ tone: 'destructive' })
  })

  it('never asks for the DNS of a local name', () => {
    expect(domainState({ status: 'active', local: true }).label).toBe('Local')
    expect(needsDns({ status: 'active', local: true })).toBe(false)
    expect(needsDns({ status: 'active', local: false })).toBe(false)
    expect(needsDns({ status: 'pending', local: false })).toBe(true)
    expect(needsDns({ status: 'misconfigured', local: false })).toBe(true)
  })
})

describe('DNS records', () => {
  it('spells the name a record answers for', () => {
    expect(recordFullName({ name: '*' }, 'example.com')).toBe('*.example.com')
    expect(recordFullName({ name: '@' }, 'example.com')).toBe('example.com')
  })

  it('says which record services need', () => {
    expect(recordPurpose({ name: '*', required: true }, 'example.com')).toBe('Every service. Required.')
    expect(recordPurpose({ name: '*', required: false }, 'example.com')).toMatch(/IPv6. Optional/)
    expect(recordPurpose({ name: '@', required: false }, 'example.com')).toMatch(/^example\.com itself\. Optional/)
  })

  it('gives the names in the parent zone of a subdomain', () => {
    expect(namesInParentZone('apps.example.com')).toEqual({ zone: 'example.com', wildcard: '*.apps', apex: 'apps' })
    expect(namesInParentZone('a.b.example.com')).toEqual({ zone: 'b.example.com', wildcard: '*.a', apex: 'a' })
    expect(namesInParentZone('example.com')).toBeNull()
  })
})

describe('certificateLook', () => {
  const now = Date.parse('2026-10-02T12:00:00Z')
  const none = { expires_at: null, error: null, retry_at: null }

  it('says when an issued certificate expires', () => {
    const look = certificateLook({ ...none, state: 'issued', expires_at: '2026-10-22T12:00:00Z' }, now)
    expect(look).toMatchObject({ label: 'Issued', tone: 'success', pulse: false })
    expect(look.detail).toBe('Expires in 20d; renewed before that.')
  })

  it('says why a request failed and when the next one is', () => {
    const look = certificateLook(
      { ...none, state: 'failed', error: 'DNS problem: NXDOMAIN looking up A for web.example.com', retry_at: '2026-10-02T12:04:50Z' },
      now,
    )
    expect(look.tone).toBe('destructive')
    expect(look.detail).toBe('DNS problem: NXDOMAIN looking up A for web.example.com. Next attempt in 4m.')
    expect(certificateLook({ ...none, state: 'failed' }, now).detail).toBe('The certificate could not be issued.')
  })

  it('tells a certificate on its way from one that never comes', () => {
    for (const state of ['pending', 'issuing'] as const) {
      expect(certificateOnItsWay({ state })).toBe(true)
      expect(certificateLook({ ...none, state }, now).pulse).toBe(true)
    }
    for (const state of ['issued', 'failed', 'local', 'disabled'] as const) {
      expect(certificateOnItsWay({ state })).toBe(false)
    }
    expect(certificateLook({ ...none, state: 'local' }, now).detail).toMatch(/no certificate/)
    expect(certificateLook({ ...none, state: 'disabled' }, now).label).toBe('Off')
  })
})
