import { describe, expect, it } from 'vitest'

import type { GitConnectionView, GitRepository } from '@/lib/api/types'

import {
  accountName,
  applicationsPageUrl,
  connectedAccounts,
  connectionHost,
  connectionLabel,
  filterRepositories,
  instanceUrl,
  instanceUrlError,
  isBranchName,
  parseHttpUrl,
  publicOrigin,
  pushAddress,
  pushDelivery,
  savedApplication,
  scopeWarning,
  serviceNameFromRepository,
  tokenExpiry,
  tokenPageUrl,
} from './git'

const repo = (full_name: string): GitRepository => ({
  id: full_name,
  full_name,
  name: full_name.split('/').pop() ?? full_name,
  owner: full_name.split('/').slice(0, -1).join('/'),
  private: false,
  archived: false,
  default_branch: 'main',
  clone_url: `https://github.com/${full_name}.git`,
  web_url: `https://github.com/${full_name}`,
  description: null,
  updated_at: null,
})

describe('parseHttpUrl (mirror of ferry_core::git::parse_http_url)', () => {
  it('splits plain http(s) URLs', () => {
    expect(parseHttpUrl('HTTPS://GitHub.com/Owner/Repo.git?x=1#frag')).toEqual({
      scheme: 'https',
      host: 'github.com',
      port: 443,
      path: '/Owner/Repo.git',
    })
    expect(parseHttpUrl('http://127.0.0.1:8929')).toEqual({ scheme: 'http', host: '127.0.0.1', port: 8929, path: '' })
    expect(parseHttpUrl('http://[::1]:3000/g/p')?.host).toBe('[::1]')
  })

  it('refuses everything else, credentials included', () => {
    for (const bad of [
      '',
      'github.com/a/b',
      'git@github.com:a/b.git',
      'ssh://git@github.com/a/b',
      '/srv/repo',
      'https://',
      'https://user@github.com/a/b',
      'https://user:tok@github.com/a/b',
      'https://github.com@evil.example/a/b',
      'https://github.com:/a/b',
      'https://github.com:0/a/b',
      'https://github.com:99999/a/b',
      'https://.github.com/a',
      'https://git hub.com/a',
    ]) {
      expect(parseHttpUrl(bad), bad).toBeNull()
    }
  })
})

describe('publicOrigin (mirror of ferry_scm::github_app::public_origin)', () => {
  it('keeps the origin of addresses on the internet', () => {
    for (const [address, origin] of [
      ['https://ferry.example.com/git/callback', 'https://ferry.example.com'],
      ['https://Ferry.Apps.Example.com:8443/', 'https://ferry.apps.example.com:8443'],
      ['http://8.8.8.8:7878/git/callback', 'http://8.8.8.8:7878'],
      ['https://[2606:4700::1111]/x', 'https://[2606:4700::1111]'],
    ] as const) {
      expect(publicOrigin(address), address).toBe(origin)
    }
  })

  it('refuses what only this machine or its network knows', () => {
    for (const local of [
      'http://localhost:7878/git/callback',
      'http://ferry.localhost:8080',
      'http://127.0.0.1:7878',
      'http://[::1]:7878',
      'http://10.0.0.5',
      'http://172.20.1.1',
      'http://192.168.1.20:7878',
      'http://169.254.10.1',
      'http://100.101.102.103',
      'http://0.0.0.0:7878',
      'http://203.0.113.7:7878',
      'http://[::ffff:10.0.0.5]',
      'http://[fd12:3456::1]',
      'http://[fe80::1]',
      'http://myserver:7878',
      'http://ferry.local',
      'http://ferry.internal',
      'http://nas.lan',
      'http://ferry.test',
      'https://ferry.example',
      'ssh://ferry.example.com',
      'not a url',
    ]) {
      expect(publicOrigin(local), local).toBeNull()
    }
  })

  it('finds where GitHub can deliver pushes to this server', () => {
    const none = { dashboard_url: null }
    const local = { dashboard_url: 'http://ferry.localhost:8080' }
    const hosted = { dashboard_url: 'https://ferry.apps.example.com' }
    expect(pushAddress('http://localhost:7878', none)).toBeNull()
    expect(pushAddress('http://localhost:7878', local)).toBeNull()
    expect(pushAddress('http://localhost:7878', undefined)).toBeNull()
    // The address the dashboard is used at first, else the server's own.
    expect(pushAddress('https://ferry.example.org', hosted)).toBe('https://ferry.example.org')
    expect(pushAddress('http://localhost:7878', hosted)).toBe('https://ferry.apps.example.com')
  })
})

describe('pushDelivery', () => {
  const on = { github_webhook_secret_set: true }
  const off = { github_webhook_secret_set: false }
  const app = { provider: 'github', push_events: true } as const
  const plain = { provider: 'github', push_events: false } as const
  const gitlab = { provider: 'gitlab', push_events: false } as const

  it('goes through the app of the account that clones the repository', () => {
    expect(pushDelivery(app, [app], off)).toBe('app')
    expect(pushDelivery(app, [app], on)).toBe('app')
  })

  it('falls back to a webhook added by hand, for GitHub only', () => {
    expect(pushDelivery(plain, [plain], on)).toBe('webhook')
    expect(pushDelivery(null, [], on)).toBe('webhook')
    expect(pushDelivery(gitlab, [gitlab, app], on)).toBe('none')
  })

  it('says none when nothing delivers pushes', () => {
    expect(pushDelivery(plain, [plain, app], off)).toBe('none')
    expect(pushDelivery(null, [plain], off)).toBe('none')
    expect(pushDelivery(null, [], off)).toBe('none')
  })

  it('does not guess for a repository whose account is not known', () => {
    expect(pushDelivery(null, [plain, app], off)).toBe('unknown')
    // Nothing loaded yet.
    expect(pushDelivery(null, undefined, off)).toBe('unknown')
    expect(pushDelivery(plain, [plain], undefined)).toBe('unknown')
  })
})

describe('accounts', () => {
  const connection = (over: Partial<GitConnectionView>): GitConnectionView => ({
    id: 'git-1',
    provider: 'gitlab',
    base_url: 'https://gitlab.com',
    auth: 'oauth',
    status: 'connected',
    account: 'tanuki',
    account_name: null,
    token_hint: null,
    scopes: [],
    token_expires_at: null,
    client_id: 'app-id',
    app_slug: null,
    app_url: null,
    manage_url: null,
    repository_selection: null,
    services: [],
    created_at: '2026-10-01T00:00:00Z',
    updated_at: '2026-10-01T00:00:00Z',
    ...over,
  })

  it('lists the accounts whose authorization is finished', () => {
    const pending = connection({ id: 'git-2', status: 'pending', account: '' })
    expect(connectedAccounts([connection({}), pending]).map((c) => c.id)).toEqual(['git-1'])
    expect(connectedAccounts(undefined)).toEqual([])
    // A connection without an account yet is named after what it is.
    expect([accountName(connection({})), accountName(pending)]).toEqual(['tanuki', 'GitLab application'])
    expect(connectionLabel(pending)).toBe('GitLab application')
  })

  it('finds the OAuth application saved for an instance', () => {
    const connected = connection({})
    const pending = connection({ id: 'git-2', status: 'pending', account: '' })
    const selfHosted = connection({ id: 'git-3', base_url: 'https://gitlab.example.com' })
    const token = connection({ id: 'git-4', auth: 'token', client_id: null })
    const all = [token, connected, pending, selfHosted]
    // The one waiting for its account first (the server looks it up first too).
    expect(savedApplication(all, 'gitlab', '')?.id).toBe('git-2')
    expect(savedApplication([token, connected], 'gitlab', 'https://gitlab.com')?.id).toBe('git-1')
    expect(savedApplication(all, 'gitlab', 'https://gitlab.example.com')?.id).toBe('git-3')
    expect(savedApplication(all, 'gitlab', 'https://other.example.com')).toBeUndefined()
    expect(savedApplication([token], 'gitlab', '')).toBeUndefined()
    expect(savedApplication(all, 'github', '')).toBeUndefined()
  })

  it('links to the page that creates the GitLab application', () => {
    expect(applicationsPageUrl()).toBe('https://gitlab.com/-/user_settings/applications')
    expect(applicationsPageUrl('gitlab.example.com/')).toBe('https://gitlab.example.com/-/user_settings/applications')
  })
})

describe('isBranchName (mirror of validate::branch)', () => {
  it('accepts what git accepts', () => {
    for (const ok of ['main', 'feature/login', 'release/1.x', 'v2', 'été']) expect(isBranchName(ok), ok).toBe(true)
    for (const bad of ['', '  ', '-x', '/a', 'a/', 'a.', 'a.lock', 'a..b', 'a//b', 'a@{b', 'a b', 'a~b', 'a:b', 'a\\b']) {
      expect(isBranchName(bad), bad).toBe(false)
    }
  })
})

describe('instances and labels', () => {
  it('reads a typed instance address', () => {
    expect(instanceUrl('  gitlab.example.com/ ')).toBe('https://gitlab.example.com')
    expect(instanceUrl('http://10.0.0.5:8929')).toBe('http://10.0.0.5:8929')
    expect(instanceUrl('  ')).toBe('')
    expect(instanceUrlError('')).toBeNull()
    expect(instanceUrlError('gitlab.example.com')).toBeNull()
    expect(instanceUrlError('https://example.com/gitlab')).toBeNull()
    expect(instanceUrlError('ssh://git@gitlab.example.com')).not.toBeNull()
    expect(instanceUrlError('https://user:pw@gitlab.example.com')).not.toBeNull()
    expect(instanceUrlError('https://gitlab.example.com/?x=1')).not.toBeNull()
  })

  it('links to the token page with the scopes Ferry needs', () => {
    expect(tokenPageUrl('github')).toBe('https://github.com/settings/tokens/new?description=Ferry&scopes=repo')
    expect(tokenPageUrl('gitlab')).toBe(
      'https://gitlab.com/-/user_settings/personal_access_tokens?name=Ferry&scopes=read_api,read_repository',
    )
    expect(tokenPageUrl('gitlab', 'gitlab.example.com/')).toBe(
      'https://gitlab.example.com/-/user_settings/personal_access_tokens?name=Ferry&scopes=read_api,read_repository',
    )
    expect(tokenPageUrl('github', '  ')).toBe('https://github.com/settings/tokens/new?description=Ferry&scopes=repo')
  })

  it('names the instance only when it is not the public one', () => {
    const cloud = { provider: 'github' as const, base_url: 'https://github.com', account: 'octocat' }
    const own = { provider: 'gitlab' as const, base_url: 'https://gitlab.example.com', account: 'tanuki' }
    expect([connectionHost(cloud), connectionLabel(cloud)]).toEqual([null, 'octocat'])
    expect([connectionHost(own), connectionLabel(own)]).toEqual(['gitlab.example.com', 'tanuki · gitlab.example.com'])
  })
})

describe('scopeWarning', () => {
  it('flags tokens that cannot read private repositories', () => {
    expect(scopeWarning({ provider: 'github', scopes: ['repo', 'read:org'] })).toBeNull()
    expect(scopeWarning({ provider: 'github', scopes: ['public_repo'] })).toMatch(/repo scope/)
    // Fine-grained tokens report no scopes: nothing to say.
    expect(scopeWarning({ provider: 'github', scopes: [] })).toBeNull()
    expect(scopeWarning({ provider: 'gitlab', scopes: ['read_api', 'read_repository'] })).toBeNull()
    expect(scopeWarning({ provider: 'gitlab', scopes: ['api'] })).toBeNull()
    expect(scopeWarning({ provider: 'gitlab', scopes: ['read_api'] })).toMatch(/read_repository/)
  })
})

describe('tokenExpiry', () => {
  const now = Date.parse('2026-10-01T12:00:00Z')
  it('describes when a token expires', () => {
    expect(tokenExpiry(null, now)).toBeNull()
    expect(tokenExpiry('not a date', now)).toBeNull()
    expect(tokenExpiry('2027-03-01T00:00:00Z', now)).toEqual({ tone: 'ok', label: 'Expires in 150 days' })
    expect(tokenExpiry('2026-10-10T12:00:00Z', now)).toEqual({ tone: 'soon', label: 'Expires in 9 days' })
    expect(tokenExpiry('2026-10-02T13:00:00Z', now)).toEqual({ tone: 'soon', label: 'Expires in 1 day' })
    expect(tokenExpiry('2026-10-01T18:00:00Z', now)).toEqual({ tone: 'soon', label: 'Expires today' })
    expect(tokenExpiry('2026-10-01T06:00:00Z', now)).toEqual({ tone: 'expired', label: 'Expired today' })
    expect(tokenExpiry('2026-09-28T12:00:00Z', now)).toEqual({ tone: 'expired', label: 'Expired 3 days ago' })
  })
})

describe('filterRepositories', () => {
  const repos = [repo('octocat/hello-world'), repo('acme/Platform-API'), repo('acme/web'), repo('octocat/api-client')]
  const names = (q: string) => filterRepositories(repos, q).map((r) => r.full_name)

  it('matches every word anywhere in the full name, keeping the order', () => {
    expect(names('')).toEqual(repos.map((r) => r.full_name))
    expect(names('  ')).toHaveLength(4)
    expect(names('api')).toEqual(['acme/Platform-API', 'octocat/api-client'])
    expect(names('ACME api')).toEqual(['acme/Platform-API'])
    expect(names('octocat/')).toEqual(['octocat/hello-world', 'octocat/api-client'])
    expect(names('nope')).toEqual([])
  })
})

describe('serviceNameFromRepository', () => {
  it('makes a valid service name', () => {
    expect(serviceNameFromRepository('hello-world')).toBe('hello-world')
    expect(serviceNameFromRepository('My_App.js')).toBe('my-app-js')
    expect(serviceNameFromRepository('2048-game')).toBe('game')
    expect(serviceNameFromRepository('--api--')).toBe('api')
    expect(serviceNameFromRepository('1234')).toBe('')
    expect(serviceNameFromRepository('a'.repeat(60))).toHaveLength(40)
    expect(serviceNameFromRepository(`${'a'.repeat(39)}-b`)).toBe('a'.repeat(39))
  })
})
