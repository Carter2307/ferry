import { describe, expect, it } from 'vitest'

import type { ServiceView } from '@/lib/api/types'

import { countByState, matchesQuery, parseStatusParam, previewServiceUrl, since, sortServices, sourceLine } from './lib'

function svc(patch: Partial<ServiceView>): ServiceView {
  return {
    id: 'srv-1',
    name: 'web',
    type: 'web_service',
    repo_url: null,
    branch: 'main',
    image: null,
    runtime: 'auto',
    root_dir: null,
    dockerfile_path: null,
    build_command: null,
    start_command: null,
    publish_dir: null,
    port: null,
    health_check_path: null,
    schedule: null,
    instances: 1,
    auto_deploy: true,
    suspended: false,
    disk_mount_path: null,
    custom_domains: [],
    deploy_hook_key: 'k',
    live_deploy_id: null,
    created_at: '2026-09-26T10:00:00Z',
    updated_at: '2026-09-26T10:00:00Z',
    state: 'live',
    url: null,
    hosts: [],
    internal_host: 'web',
    internal_port: null,
    env_groups: [],
    latest_deploy: null,
    deploy_hook_path: '/hooks/deploy/srv-1?key=k',
    ...patch,
  }
}

const deployAt = (created_at: string): ServiceView['latest_deploy'] => ({
  id: 'dep-1',
  service_id: 'srv-1',
  status: 'live',
  trigger: 'manual',
  source: { kind: 'image', image: 'x' },
  commit_sha: null,
  commit_message: null,
  image: null,
  port: null,
  error: null,
  created_at,
  started_at: null,
  finished_at: null,
})

describe('services list helpers', () => {
  it('source line', () => {
    expect(sourceLine(svc({ repo_url: 'https://github.com/you/app.git', branch: 'dev' }))).toBe('app@dev')
    expect(sourceLine(svc({ image: 'nginx:alpine' }))).toBe('nginx:alpine')
    expect(sourceLine(svc({}))).toMatch(/ferry up/)
  })

  it('search matches name, image, type and domains', () => {
    const s = svc({ name: 'api', image: 'ghcr.io/me/api:1', custom_domains: ['api.example.com'] })
    expect(matchesQuery(s, 'API')).toBe(true)
    expect(matchesQuery(s, 'ghcr')).toBe(true)
    expect(matchesQuery(s, 'web service')).toBe(true)
    expect(matchesQuery(s, 'example.com')).toBe(true)
    expect(matchesQuery(s, 'nope')).toBe(false)
    expect(matchesQuery(s, '  ')).toBe(true)
  })

  it('sorts by name or by last deploy (never-deployed last)', () => {
    const a = svc({ id: 'a', name: 'alpha', latest_deploy: deployAt('2026-09-26T09:00:00Z') })
    const b = svc({ id: 'b', name: 'bravo', latest_deploy: deployAt('2026-09-26T11:00:00Z') })
    const c = svc({ id: 'c', name: 'charlie' })
    expect(sortServices([c, b, a], 'name').map((s) => s.name)).toEqual(['alpha', 'bravo', 'charlie'])
    expect(sortServices([c, a, b], 'last_deploy').map((s) => s.name)).toEqual(['bravo', 'alpha', 'charlie'])
  })

  it('counts states and parses the status param', () => {
    const counts = countByState([svc({ state: 'live' }), svc({ state: 'failed' }), svc({ state: 'live' })])
    expect(counts.live).toBe(2)
    expect(counts.failed).toBe(1)
    expect(parseStatusParam('failed,bogus,live')).toEqual(['live', 'failed'])
    expect(parseStatusParam(null)).toEqual([])
  })

  it('previews the default URL like Config::url_for_host', () => {
    const info = { base_domain: 'localhost', proxy_url: 'http://127.0.0.1:19801', tls_enabled: false, dashboard_url: null }
    expect(previewServiceUrl('api', info)).toBe('http://api.localhost:19801')
    expect(previewServiceUrl('api', { ...info, base_domain: 'example.com', tls_enabled: true })).toBe('https://api.example.com')
    expect(previewServiceUrl('api', { ...info, proxy_url: 'http://0.0.0.0:80' })).toBe('http://api.localhost')
  })

  it('never shows future times for fresh deploys', () => {
    const now = Date.parse('2026-09-26T10:00:00Z')
    expect(since('2026-09-26T10:00:11Z', now)).toBe('just now')
    expect(since('2026-09-26T09:57:00Z', now)).toBe('3m ago')
  })
})
