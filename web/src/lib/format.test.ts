import { describe, expect, it } from 'vitest'

import {
  bytes,
  deploySourceLabel,
  displayUrl,
  duration,
  durationBetween,
  percent,
  plural,
  relativeTime,
  repoName,
  shortDateTime,
  shortId,
  shortSha,
  stripAnsi,
} from './format'

const NOW = Date.parse('2026-09-26T14:00:00Z')

describe('relativeTime', () => {
  it('formats past times', () => {
    expect(relativeTime('2026-09-26T13:59:58Z', NOW)).toBe('just now')
    expect(relativeTime('2026-09-26T13:59:30Z', NOW)).toBe('30s ago')
    expect(relativeTime('2026-09-26T13:55:00Z', NOW)).toBe('5m ago')
    expect(relativeTime('2026-09-26T11:00:00Z', NOW)).toBe('3h ago')
    expect(relativeTime('2026-09-22T14:00:00Z', NOW)).toBe('4d ago')
  })
  it('formats future times and invalid input', () => {
    expect(relativeTime('2026-09-26T14:10:00Z', NOW)).toBe('in 10m')
    expect(relativeTime(null, NOW)).toBe('—')
    expect(relativeTime('not a date', NOW)).toBe('—')
  })
  it('falls back to a date after 30 days', () => {
    expect(relativeTime('2026-07-01T12:00:00Z', NOW)).toBe('Jul 1')
    expect(relativeTime('2025-07-01T12:00:00Z', NOW)).toBe('Jul 1, 2025')
  })
})

describe('duration', () => {
  it('formats ms, seconds, minutes and hours', () => {
    expect(duration(850)).toBe('850ms')
    expect(duration(12_400)).toBe('12s')
    expect(duration(184_000)).toBe('3m 04s')
    expect(duration(3_720_000)).toBe('1h 02m')
    expect(duration(-1)).toBe('—')
    expect(duration(null)).toBe('—')
  })
  it('computes spans, open-ended spans use now', () => {
    expect(durationBetween('2026-09-26T13:59:00Z', '2026-09-26T13:59:42Z')).toBe('42s')
    expect(durationBetween('2026-09-26T13:58:00Z', null, NOW)).toBe('2m 00s')
    expect(durationBetween(null)).toBe('—')
  })
})

describe('bytes / percent', () => {
  it('uses binary units', () => {
    expect(bytes(0)).toBe('0 B')
    expect(bytes(512)).toBe('512 B')
    expect(bytes(1536)).toBe('1.5 KiB')
    expect(bytes(52_428_800)).toBe('50.0 MiB')
    expect(bytes(200 * 1024 * 1024)).toBe('200 MiB')
    expect(bytes(null)).toBe('—')
  })
  it('formats percentages', () => {
    expect(percent(12.345)).toBe('12.3%')
    expect(percent(undefined)).toBe('—')
  })
})

describe('ids and names', () => {
  it('shortens ids keeping the prefix', () => {
    expect(shortId('dep-fd8a901ce5724b6ea476')).toBe('dep-fd8a901c')
    expect(shortId('abcdef0123456789')).toBe('abcdef01')
    expect(shortId(null)).toBe('—')
  })
  it('shortens shas and pluralizes', () => {
    expect(shortSha('c6410f5f8432971f4394bc4cf77cf98476177108')).toBe('c6410f5')
    expect(plural(1, 'instance')).toBe('1 instance')
    expect(plural(3, 'instance')).toBe('3 instances')
  })
  it('derives repo names and display urls', () => {
    expect(repoName('https://github.com/acme/my-app.git')).toBe('my-app')
    expect(repoName('/srv/git/api-src/')).toBe('api-src')
    expect(repoName('git@github.com:acme/tool.git')).toBe('tool')
    expect(displayUrl('http://api.localhost:19801/')).toBe('api.localhost:19801')
  })
  it('labels deploy sources', () => {
    expect(deploySourceLabel({ kind: 'git', repo_url: '/x/api-src', branch: 'main', commit: null })).toBe('api-src@main')
    expect(deploySourceLabel({ kind: 'image', image: 'busybox:stable' })).toBe('busybox:stable')
    expect(deploySourceLabel({ kind: 'archive', path: '/u/1.tar.gz' })).toBe('Uploaded archive')
  })
})

describe('stripAnsi', () => {
  it('removes color codes and OSC sequences', () => {
    expect(stripAnsi('\u001b[31merror\u001b[0m: boom')).toBe('error: boom')
    expect(stripAnsi('\u001b[1;32m✓\u001b[39;49m ok')).toBe('✓ ok')
    expect(stripAnsi('\u001b]8;;http://x\u0007link\u001b]8;;\u0007')).toBe('link')
    expect(stripAnsi('plain')).toBe('plain')
  })
})

describe('shortDateTime', () => {
  it('formats month, day and 24h time', () => {
    const d = new Date(2026, 8, 26, 20, 4, 7)
    expect(shortDateTime(d, new Date(2026, 0, 1))).toBe('Sep 26, 20:04:07')
  })
  it('adds the year when it differs from now', () => {
    const d = new Date(2025, 11, 31, 9, 0, 0)
    expect(shortDateTime(d, new Date(2026, 0, 1))).toBe('Dec 31 2025, 09:00:00')
  })
  it('returns a dash for missing values', () => {
    expect(shortDateTime(null)).toBe('—')
  })
})
