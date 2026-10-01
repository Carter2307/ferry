import { describe, expect, it } from 'vitest'

import {
  branchError,
  buildFormFrom,
  buildPatch,
  checkDomain,
  describeSchedule,
  repoUrlError,
  scalingFormFrom,
  scalingPatch,
  scheduleError,
  validateBuildForm,
  validateScalingForm,
  type BuildForm,
} from './forms'

const gitService = {
  repo_url: 'https://github.com/acme/api.git',
  git_connection_id: null as string | null,
  branch: 'main',
  image: null,
  runtime: 'node' as const,
  root_dir: null,
  dockerfile_path: null,
  build_command: 'npm run build',
  start_command: null,
  publish_dir: null,
  port: null,
  health_check_path: '/healthz',
  auto_deploy: true,
}

const imageService = { ...gitService, repo_url: null, image: 'nginx:alpine', runtime: 'image' as const, build_command: null }

describe('buildPatch', () => {
  it('is empty when nothing changed', () => {
    const f = buildFormFrom(gitService)
    expect(buildPatch(f, { ...f }, 'web_service')).toEqual({})
  })

  it('sends only changed fields, trimmed; empty clears; port "" → 0', () => {
    const initial = buildFormFrom({ ...gitService, port: 3000 })
    const next: BuildForm = { ...initial, build_command: '  ', start_command: ' node server.js ', port: '', auto_deploy: false }
    expect(buildPatch(initial, next, 'web_service')).toEqual({
      build_command: '',
      start_command: 'node server.js',
      port: 0,
      auto_deploy: false,
    })
  })

  it('git → image clears repo_url in the same request and leaves the runtime to the server', () => {
    const initial = buildFormFrom(gitService)
    const next: BuildForm = { ...initial, source: 'image', image: 'ghcr.io/acme/api:1' }
    expect(buildPatch(initial, next, 'web_service')).toEqual({ image: 'ghcr.io/acme/api:1', repo_url: '' })
  })

  it('image → git clears the image and sets the chosen runtime', () => {
    const initial = buildFormFrom(imageService)
    expect(initial.runtime).toBe('auto')
    const next: BuildForm = { ...initial, source: 'git', repo_url: 'git@github.com:acme/web.git', branch: 'prod' }
    expect(buildPatch(initial, next, 'web_service')).toEqual({
      repo_url: 'git@github.com:acme/web.git',
      image: '',
      branch: 'prod',
      runtime: 'auto',
    })
  })

  it('→ upload clears the current source', () => {
    const initial = buildFormFrom(gitService)
    expect(buildPatch(initial, { ...initial, source: 'upload' }, 'web_service')).toEqual({ repo_url: '' })
    const img = buildFormFrom(imageService)
    expect(buildPatch(img, { ...img, source: 'upload' }, 'web_service')).toEqual({ image: '', runtime: 'auto' })
  })

  it('ignores fields that do not apply (port on a worker, build command on docker)', () => {
    const initial = buildFormFrom(gitService)
    expect(buildPatch(initial, { ...initial, port: '8080' }, 'background_worker')).toEqual({})
    const docker = buildFormFrom({ ...gitService, runtime: 'docker' })
    expect(buildPatch(docker, { ...docker, build_command: 'make' }, 'web_service')).toEqual({})
  })

  it('selects, replaces and removes the git connection explicitly', () => {
    const initial = buildFormFrom(gitService)
    expect(initial.git_connection_id).toBe('')
    expect(buildPatch(initial, { ...initial, git_connection_id: 'git-a' }, 'web_service')).toEqual({ git_connection_id: 'git-a' })
    const connected = buildFormFrom({ ...gitService, git_connection_id: 'git-a' })
    expect(buildPatch(connected, { ...connected, git_connection_id: 'git-b' }, 'web_service')).toEqual({ git_connection_id: 'git-b' })
    // '' removes it.
    expect(buildPatch(connected, { ...connected, git_connection_id: '' }, 'web_service')).toEqual({ git_connection_id: '' })
    // Untouched, it is left to the server (which keeps it while the repository stays on its host).
    const moved = { ...connected, repo_url: 'https://github.com/acme/web.git' }
    expect(buildPatch(connected, moved, 'web_service')).toEqual({ repo_url: 'https://github.com/acme/web.git' })
    // Leaving git doesn't mention it either: the server drops it with the repository.
    expect(buildPatch(connected, { ...connected, source: 'image', image: 'nginx' }, 'web_service')).toEqual({
      image: 'nginx',
      repo_url: '',
    })
  })
})

describe('validateBuildForm', () => {
  it('requires the fields of the chosen source', () => {
    const f = buildFormFrom(gitService)
    expect(validateBuildForm({ ...f, repo_url: '' }, 'web_service').repo_url).toBeTruthy()
    expect(validateBuildForm({ ...f, source: 'image', image: '' }, 'web_service').image).toBeTruthy()
    expect(validateBuildForm({ ...f, source: 'upload', repo_url: '' }, 'web_service')).toEqual({})
  })

  it('checks paths, port and health check', () => {
    const f = buildFormFrom(gitService)
    const e = validateBuildForm({ ...f, root_dir: '../x', port: '70000', health_check_path: 'healthz' }, 'web_service')
    expect(Object.keys(e).sort()).toEqual(['health_check_path', 'port', 'root_dir'])
  })

  it('a git account only clones repositories of its own host', () => {
    const account = {
      id: 'git-a',
      provider: 'github' as const,
      base_url: 'https://github.com',
      account: 'octocat',
      account_name: null,
      token_hint: '…0001',
      scopes: [],
      token_expires_at: null,
      services: [],
      created_at: '2026-10-01T00:00:00Z',
      updated_at: '2026-10-01T00:00:00Z',
    }
    const f = { ...buildFormFrom(gitService), git_connection_id: 'git-a' }
    expect(validateBuildForm(f, 'web_service', [account])).toEqual({})
    for (const repo_url of ['https://gitlab.com/acme/api.git', 'git@github.com:acme/api.git', '/srv/git/api']) {
      const e = validateBuildForm({ ...f, repo_url }, 'web_service', [account])
      expect(e.git_connection_id, repo_url).toMatch(/octocat only clones http\(s\) repositories on github\.com/)
    }
    // Without the list (still loading), or with an account that is gone, the server decides.
    expect(validateBuildForm({ ...f, repo_url: '/srv/git/api' }, 'web_service')).toEqual({})
    expect(validateBuildForm({ ...f, repo_url: '/srv/git/api', git_connection_id: 'git-x' }, 'web_service', [account])).toEqual({})
    // No account, nothing to check; other sources ignore it.
    expect(validateBuildForm({ ...f, repo_url: '/srv/git/api', git_connection_id: '' }, 'web_service', [account])).toEqual({})
    expect(validateBuildForm({ ...f, source: 'image', image: 'nginx' }, 'web_service', [account])).toEqual({})
  })
})

describe('repoUrlError / branchError', () => {
  it('accepts the forms validate::repo_url accepts', () => {
    for (const u of ['https://github.com/a/b', 'ssh://git@host/a.git', 'git@github.com:a/b.git', '/srv/git/app', 'file:///srv/app']) {
      expect(repoUrlError(u)).toBeNull()
    }
  })
  it('rejects relative paths, bad schemes and helpers', () => {
    for (const u of ['app', './app', 'ftp://x/y', 'ext::sh -c x', '-oProxy', 'file://relative']) {
      expect(repoUrlError(u)).not.toBeNull()
    }
  })
  it('validates branches', () => {
    expect(branchError('feature/x')).toBeNull()
    for (const b of ['', 'a..b', 'x.lock', 'has space', '-x', 'a:b']) expect(branchError(b)).not.toBeNull()
  })
})

describe('scaling', () => {
  it('validates instances and the disk rules', () => {
    const f = scalingFormFrom({ instances: 2, disk_mount_path: null })
    expect(validateScalingForm(f, 'web_service')).toEqual({})
    expect(validateScalingForm({ ...f, instances: '0' }, 'web_service').instances).toBeTruthy()
    expect(validateScalingForm({ ...f, instances: '51' }, 'web_service').instances).toBeTruthy()
    expect(validateScalingForm({ ...f, disk_mount_path: '/data' }, 'web_service').instances).toMatch(/1 instance/)
    expect(validateScalingForm({ instances: '1', disk_mount_path: 'data' }, 'web_service').disk_mount_path).toBeTruthy()
  })
  it('patches only what changed', () => {
    const f = scalingFormFrom({ instances: 1, disk_mount_path: '/data' })
    expect(scalingPatch(f, { ...f }, 'web_service')).toEqual({})
    expect(scalingPatch(f, { instances: '1', disk_mount_path: '' }, 'web_service')).toEqual({ disk_mount_path: '' })
    expect(scalingPatch(f, { instances: '3', disk_mount_path: '/data' }, 'static_site')).toEqual({ instances: 3 })
  })
})

describe('cron schedule', () => {
  it('accepts 5 fields and aliases', () => {
    for (const s of ['*/15 * * * *', '0 3 * * *', '@daily', '@Hourly', '0 9 * * 1-5']) expect(scheduleError(s)).toBeNull()
    for (const s of ['', '* * * *', '0 0 * * * *', '@often']) expect(scheduleError(s)).not.toBeNull()
  })
  it('describes common schedules', () => {
    expect(describeSchedule('0 3 * * *')).toBe('Every day at 03:00 UTC')
    expect(describeSchedule('*/15 * * * *')).toBe('Every 15 minutes')
    expect(describeSchedule('0 9 * * 1-5')).toBe('Weekdays at 09:00 UTC')
    expect(describeSchedule('@hourly')).toBe('Every hour, on the hour')
    expect(describeSchedule('5 4 1,15 * *')).toBeNull()
  })
})

describe('checkDomain', () => {
  it('normalizes and validates like validate::domain', () => {
    expect(checkDomain(' App.Example.com. ')).toEqual({ domain: 'app.example.com', error: null })
    expect(checkDomain('https://www.example.test/path').domain).toBe('www.example.test')
    expect(checkDomain('localhost').error).toBeTruthy()
    expect(checkDomain('10.0.0.1').error).toMatch(/IP/)
    expect(checkDomain('-bad.example.com').error).toBeTruthy()
  })
})
