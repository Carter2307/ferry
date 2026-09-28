import { describe, expect, it } from 'vitest'

import type { ServerInfo } from '@/lib/api/types'

import {
  chooseLimit,
  cpuLimitError,
  DEFAULT_LIMIT,
  describeLimitsPatch,
  effectiveCpuText,
  effectiveMemoryText,
  formatCpus,
  formatMemoryMb,
  limitError,
  limitFieldFrom,
  limitOptions,
  limitsFormFrom,
  limitsPatch,
  limitValue,
  memoryHostWarning,
  memoryLimitError,
  parseCpus,
  parseMemoryMb,
  readLimit,
  showLimitError,
  validateLimitsForm,
} from './resources'

const info: ServerInfo = {
  version: '0.1.0',
  base_domain: 'localhost',
  proxy_url: 'http://localhost:8080',
  tls_enabled: false,
  dashboard_url: null,
  github_webhook_enabled: false,
  docker_version: '28.0.0',
  default_memory_limit_mb: 512,
  default_cpu_limit: 1,
  docker_cpus: 2,
  docker_memory_bytes: 4 * 1024 ** 3,
}

describe('memory sizes (same cases as resources.rs)', () => {
  it('parses', () => {
    expect(parseMemoryMb('512')).toBe(512)
    expect(parseMemoryMb('512M')).toBe(512)
    expect(parseMemoryMb(' 512mib ')).toBe(512)
    expect(parseMemoryMb('1G')).toBe(1024)
    expect(parseMemoryMb('1.5GB')).toBe(1536)
    expect(parseMemoryMb('2gib')).toBe(2048)
    expect(parseMemoryMb('1.5 GiB')).toBe(1536)
    expect(parseMemoryMb('1T')).toBe(1024 * 1024)
    expect(parseMemoryMb('2048k')).toBe(2)
    expect(parseMemoryMb('0')).toBe(0)
    for (const bad of ['', 'abc', '-1G', '1X', '1.2.3G', '1k', '.', 'G']) {
      expect(parseMemoryMb(bad), bad).toBeNull()
    }
  })

  it('formats', () => {
    expect(formatMemoryMb(512)).toBe('512 MiB')
    expect(formatMemoryMb(1024)).toBe('1 GiB')
    expect(formatMemoryMb(1536)).toBe('1.5 GiB')
    expect(formatMemoryMb(1500)).toBe('1500 MiB')
    expect(formatMemoryMb(1024 * 1024)).toBe('1024 GiB')
  })
})

describe('CPU amounts (same cases as resources.rs)', () => {
  it('parses', () => {
    expect(parseCpus('0.5')).toBe(0.5)
    expect(parseCpus('2')).toBe(2)
    expect(parseCpus('500m')).toBe(0.5)
    expect(parseCpus('1500m')).toBe(1.5)
    expect(parseCpus('1 cpu')).toBe(1)
    expect(parseCpus('4cpus')).toBe(4)
    expect(parseCpus('0.333')).toBe(0.33)
    expect(parseCpus('0')).toBe(0)
    for (const bad of ['', 'x', '-1', 'inf', 'NaN', 'm']) {
      expect(parseCpus(bad), bad).toBeNull()
    }
  })

  it('formats', () => {
    expect(formatCpus(0.5)).toBe('0.5 CPU')
    expect(formatCpus(0.25)).toBe('0.25 CPU')
    expect(formatCpus(1)).toBe('1 CPU')
    expect(formatCpus(2)).toBe('2 CPUs')
  })
})

describe('ranges', () => {
  it('matches ferry_core::resources::validate', () => {
    expect(memoryLimitError(16)).toBeNull()
    expect(memoryLimitError(1024 * 1024)).toBeNull()
    expect(memoryLimitError(8)).toMatch(/16 MiB and 1024 GiB/)
    expect(memoryLimitError(1024 * 1024 + 1)).not.toBeNull()
    expect(cpuLimitError(0.01)).toBeNull()
    expect(cpuLimitError(512)).toBeNull()
    expect(cpuLimitError(0)).not.toBeNull()
    expect(cpuLimitError(512.5)).not.toBeNull()
    expect(cpuLimitError(Number.NaN)).not.toBeNull()
  })

  it('caps CPUs at the Docker host when known', () => {
    expect(cpuLimitError(2, 2)).toBeNull()
    expect(cpuLimitError(3, 2)).toMatch(/Docker host has 2 CPUs/)
    expect(cpuLimitError(3, null)).toBeNull()
  })
})

describe('limit fields', () => {
  it('maps stored values to a preset, custom, or the default', () => {
    expect(limitFieldFrom('memory', null)).toEqual(DEFAULT_LIMIT)
    expect(limitFieldFrom('memory', 0)).toEqual(DEFAULT_LIMIT)
    expect(limitFieldFrom('memory', 1024)).toEqual({ choice: '1024', custom: '' })
    expect(limitFieldFrom('memory', 1536)).toEqual({ choice: 'custom', custom: '1.5 GiB' })
    // 1088 MiB formats as "1.06 GiB", which would not round-trip.
    expect(limitFieldFrom('memory', 1088)).toEqual({ choice: 'custom', custom: '1088 MiB' })
    expect(limitFieldFrom('cpu', 0.5)).toEqual({ choice: '0.5', custom: '' })
    expect(limitFieldFrom('cpu', 0.75)).toEqual({ choice: 'custom', custom: '0.75' })
  })

  it('reads and validates presets and custom values', () => {
    expect(readLimit('memory', DEFAULT_LIMIT)).toEqual({ ok: true, value: null })
    expect(readLimit('memory', { choice: '2048', custom: '' })).toEqual({ ok: true, value: 2048 })
    expect(readLimit('memory', { choice: 'custom', custom: '768M' })).toEqual({ ok: true, value: 768 })
    expect(readLimit('memory', { choice: 'custom', custom: '1.5G' })).toEqual({ ok: true, value: 1536 })
    expect(limitError('memory', { choice: 'custom', custom: '' })).toMatch(/Enter a size/)
    expect(limitError('memory', { choice: 'custom', custom: 'lots' })).toMatch(/not a size/)
    expect(limitError('memory', { choice: 'custom', custom: '8M' })).toMatch(/between 16 MiB/)
    expect(limitError('memory', { choice: 'custom', custom: '0' })).toMatch(/between 16 MiB/)
    expect(readLimit('cpu', { choice: 'custom', custom: '750m' })).toEqual({ ok: true, value: 0.75 })
    expect(limitError('cpu', { choice: 'custom', custom: '0.001' })).toMatch(/between 0.01 and 512/)
    expect(limitError('cpu', { choice: 'custom', custom: '3' }, 2)).toMatch(/Docker host/)
    expect(limitError('cpu', { choice: '4', custom: '' }, 2)).toMatch(/Docker host/)
    expect(limitValue('cpu', { choice: 'custom', custom: 'nope' })).toBeNull()
  })

  it('pre-fills Custom… with the previous preset', () => {
    expect(chooseLimit('memory', { choice: '2048', custom: '' }, 'custom')).toEqual({ choice: 'custom', custom: '2 GiB' })
    expect(chooseLimit('cpu', { choice: '0.5', custom: '' }, 'custom')).toEqual({ choice: 'custom', custom: '0.5' })
    expect(chooseLimit('cpu', DEFAULT_LIMIT, 'custom')).toEqual({ choice: 'custom', custom: '' })
    // An edited custom value is kept when switching away and back.
    const edited = { choice: 'custom', custom: '3' }
    const away = chooseLimit('cpu', edited, '1')
    expect(away).toEqual({ choice: '1', custom: '3' })
    expect(chooseLimit('cpu', away, 'custom')).toEqual(edited)
  })
})

describe('options and labels', () => {
  it('labels the server default from /api/v1/info', () => {
    const mem = limitOptions('memory', info)
    expect(mem.map((o) => o.label)).toEqual([
      'Server default (512 MiB)',
      '256 MiB',
      '512 MiB',
      '1 GiB',
      '2 GiB',
      '4 GiB',
      '8 GiB',
      'Custom…',
    ])
    expect(mem.find((o) => o.value === '8192')?.note).toBe('more than the host')
    expect(mem.find((o) => o.value === '8192')?.disabled).toBeFalsy()

    const cpu = limitOptions('cpu', info)
    expect(cpu.map((o) => o.label)).toEqual(['Server default (1 CPU)', '0.25 CPU', '0.5 CPU', '1 CPU', '2 CPUs', '4 CPUs', 'Custom…'])
    expect(cpu.find((o) => o.value === '4')?.disabled).toBe(true)
    expect(cpu.find((o) => o.value === '2')?.disabled).toBe(false)

    const unlimited = { ...info, default_memory_limit_mb: 0, default_cpu_limit: 0 }
    expect(limitOptions('memory', unlimited)[0]?.label).toBe('Server default (Unlimited)')
    expect(limitOptions('cpu', unlimited)[0]?.label).toBe('Server default (Unlimited)')
    expect(limitOptions('cpu', undefined)[0]?.label).toBe('Server default')
  })

  it('describes effective limits', () => {
    expect(effectiveMemoryText(null, info)).toBe('Server default (512 MiB)')
    expect(effectiveMemoryText(2048, info)).toBe('2 GiB')
    expect(effectiveCpuText(null, { ...info, default_cpu_limit: 0 })).toBe('Server default (Unlimited)')
    expect(effectiveCpuText(0.5, undefined)).toBe('0.5 CPU')
    expect(effectiveCpuText(null, undefined)).toBe('Server default')
  })

  it('warns when a memory limit exceeds the host', () => {
    expect(memoryHostWarning(8192, info)).toMatch(/4.0 GiB/)
    expect(memoryHostWarning(2048, info)).toBeNull()
    expect(memoryHostWarning(null, info)).toBeNull()
    expect(memoryHostWarning(8192, { ...info, docker_memory_bytes: null })).toBeNull()
  })
})

describe('limits form', () => {
  const base = limitsFormFrom({ memory_limit_mb: 1024, cpu_limit: null })

  it('patches only what changed, 0 back to the default', () => {
    expect(limitsPatch(base, base)).toEqual({})
    expect(limitsPatch(base, { ...base, memory: 'default' })).toEqual({ memory_limit_mb: 0 })
    expect(limitsPatch(base, { ...base, cpu: '0.5' })).toEqual({ cpu_limit: 0.5 })
    // Same value typed as a custom size: nothing to save.
    expect(limitsPatch(base, { ...base, memory: 'custom', memoryCustom: '1G' })).toEqual({})
    expect(limitsPatch(base, { ...base, memory: 'custom', memoryCustom: '1.5G', cpu: 'custom', cpuCustom: '1500m' })).toEqual({
      memory_limit_mb: 1536,
      cpu_limit: 1.5,
    })
  })

  it('validates, capping only a changed CPU limit at the host', () => {
    expect(validateLimitsForm(base, base, 2)).toEqual({})
    expect(validateLimitsForm({ ...base, memory: 'custom', memoryCustom: '4M' }, base, 2).memory).toMatch(/16 MiB/)
    expect(validateLimitsForm({ ...base, cpu: '4' }, base, 2).cpu).toMatch(/Docker host/)
    // A stored 4 CPUs on a 2-CPU host doesn't block saving the memory limit.
    const stored = limitsFormFrom({ memory_limit_mb: null, cpu_limit: 4 })
    expect(validateLimitsForm({ ...stored, memory: '2048' }, stored, 2)).toEqual({})
  })

  it('shows errors once changed, but not for a Custom… input still empty', () => {
    const stored = { choice: '1024', custom: '' }
    expect(showLimitError(stored, stored, false)).toBe(false)
    expect(showLimitError({ choice: 'custom', custom: '' }, stored, false)).toBe(false)
    expect(showLimitError({ choice: 'custom', custom: '' }, stored, true)).toBe(true)
    expect(showLimitError({ choice: 'custom', custom: '8M' }, stored, false)).toBe(true)
    expect(showLimitError({ choice: 'default', custom: '' }, stored, false)).toBe(true)
  })

  it('summarizes a patch', () => {
    expect(describeLimitsPatch({ memory_limit_mb: 1024 })).toBe('Memory 1 GiB')
    expect(describeLimitsPatch({ memory_limit_mb: 0, cpu_limit: 0.5 })).toBe('Memory server default · CPU 0.5 CPU')
    expect(describeLimitsPatch({ cpu_limit: 0 })).toBe('CPU server default')
  })
})
