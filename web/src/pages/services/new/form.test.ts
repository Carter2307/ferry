import { describe, expect, it } from 'vitest'

import {
  fieldForServerError,
  INITIAL_FORM,
  normalizeDomain,
  toCreateRequest,
  validateBranch,
  validateDockerfilePath,
  validateForm,
  validateName,
  validateRepoUrl,
  validateSchedule,
  visibleFields,
  type NewServiceForm,
} from './form'

const none = { services: [], datastores: [] }
const form = (patch: Partial<NewServiceForm>): NewServiceForm => ({ ...INITIAL_FORM, ...patch })

describe('validateName (mirrors validate::resource_name)', () => {
  it('accepts DNS-label names', () => {
    expect(validateName('web', none)).toBeNull()
    expect(validateName('my-api-2', none)).toBeNull()
    expect(validateName('srv-web', none)).toBeNull()
  })
  it('rejects bad shapes, reserved names and ids', () => {
    expect(validateName('', none)).not.toBeNull()
    expect(validateName('2web', none)).not.toBeNull()
    expect(validateName('Web', none)).not.toBeNull()
    expect(validateName('web-', none)).not.toBeNull()
    expect(validateName('ferry', none)).not.toBeNull()
    expect(validateName('a'.repeat(41), none)).not.toBeNull()
    expect(validateName('srv-0123456789abcdef0123', none)).not.toBeNull()
  })
  it('rejects names taken by services or datastores', () => {
    expect(validateName('api', { services: ['api'], datastores: [] })).toMatch(/service/)
    expect(validateName('app-db', { services: [], datastores: ['app-db'] })).toMatch(/datastore/)
  })
})

describe('source validators', () => {
  it('repo URLs', () => {
    for (const ok of ['https://github.com/a/b', 'git@github.com:a/b.git', '/abs/path', 'file:///abs/path', 'ssh://git@github.com:22/a/b']) {
      expect(validateRepoUrl(ok), ok).toBeNull()
    }
    for (const bad of ['', './rel', 'rel/path', '--upload-pack=x', 'ftp://x/y', 'ext::sh -c touch%S', 'file://x', 'file:///../x', '/a/../b', 'C:x']) {
      expect(validateRepoUrl(bad), bad).not.toBeNull()
    }
  })
  it('branches', () => {
    expect(validateBranch('feature/x-1')).toBeNull()
    expect(validateBranch('')).toBeNull()
    expect(validateBranch('-evil')).not.toBeNull()
    expect(validateBranch('a..b')).not.toBeNull()
  })
  it('dockerfile paths stay inside the checkout', () => {
    expect(validateDockerfilePath('../Dockerfile', 'apps/api')).toBeNull()
    expect(validateDockerfilePath('../../../Dockerfile', 'apps/api')).not.toBeNull()
    expect(validateDockerfilePath('/Dockerfile', '')).not.toBeNull()
  })
})

describe('validateSchedule', () => {
  it('accepts 5 fields and aliases', () => {
    expect(validateSchedule('*/15 * * * *')).toBeNull()
    expect(validateSchedule('@daily')).toBeNull()
    expect(validateSchedule('@midnight')).toBeNull()
  })
  it('rejects empty, wrong field counts and unknown aliases', () => {
    expect(validateSchedule('')).not.toBeNull()
    expect(validateSchedule('* * * *')).not.toBeNull()
    expect(validateSchedule('0 * * * * *')).not.toBeNull()
    expect(validateSchedule('@often')).not.toBeNull()
  })
})

describe('normalizeDomain', () => {
  it('normalizes and rejects IPs', () => {
    expect(normalizeDomain('App.Example.COM.')).toEqual({ domain: 'app.example.com' })
    expect('error' in normalizeDomain('127.0.0.1')).toBe(true)
    expect('error' in normalizeDomain('localhost')).toBe(true)
  })
})

describe('visibleFields / validateForm', () => {
  it('cron jobs need a schedule and have no instances, port or domains', () => {
    const v = visibleFields({ type: 'cron_job', source: 'image', runtime: 'auto' })
    expect(v.schedule).toBe(true)
    expect(v.instances).toBe(false)
    expect(v.port).toBe(false)
    expect(v.domains).toBe(false)
    const errors = validateForm(form({ type: 'cron_job', name: 'job', source: 'image', image: 'busybox' }), none)
    expect(errors.schedule).toBeDefined()
  })
  it('a disk limits instances to 1', () => {
    const errors = validateForm(
      form({ name: 'db-app', source: 'image', image: 'nginx', diskMountPath: '/data', instances: '2' }),
      none,
    )
    expect(errors.instances).toMatch(/disk/)
  })
  it('an invalid domain draft blocks submit', () => {
    const errors = validateForm(form({ name: 'web', source: 'image', image: 'nginx', domainDraft: 'not a domain' }), none)
    expect(errors.domains).toBeDefined()
  })
})

describe('toCreateRequest', () => {
  it('sends only fields that apply to the type and source', () => {
    const body = toCreateRequest(
      form({
        type: 'cron_job',
        name: 'nightly',
        source: 'image',
        image: ' busybox:stable ',
        schedule: '0 3 * * *',
        startCommand: 'echo hi',
        port: '8080',
        instances: '3',
        domains: ['a.example.com'],
      }),
      [],
    )
    expect(body).toEqual({
      name: 'nightly',
      type: 'cron_job',
      image: 'busybox:stable',
      start_command: 'echo hi',
      schedule: '0 3 * * *',
      deploy: true,
    })
  })
  it('git web service with env, groups, domains (incl. a pending draft)', () => {
    const body = toCreateRequest(
      form({
        name: 'web',
        repoUrl: 'https://github.com/a/b',
        branch: '',
        port: '3000',
        instances: '2',
        domains: ['a.example.com'],
        domainDraft: 'b.example.com',
        envGroups: ['shared'],
        autoDeploy: false,
      }),
      [{ key: 'A', value: '1' }],
    )
    expect(body).toMatchObject({
      name: 'web',
      type: 'web_service',
      repo_url: 'https://github.com/a/b',
      branch: 'main',
      auto_deploy: false,
      runtime: 'auto',
      port: 3000,
      instances: 2,
      custom_domains: ['a.example.com', 'b.example.com'],
      env: [{ key: 'A', value: '1' }],
      env_groups: ['shared'],
      deploy: true,
    })
    expect(body).not.toHaveProperty('image')
    expect(body).not.toHaveProperty('schedule')
  })
  it('upload services are created without a deploy', () => {
    const body = toCreateRequest(form({ name: 'app', source: 'upload' }), [])
    expect(body.deploy).toBe(false)
    expect(body).not.toHaveProperty('repo_url')
    expect(body).not.toHaveProperty('auto_deploy')
  })
})

describe('fieldForServerError', () => {
  it('maps server messages to fields', () => {
    expect(fieldForServerError("name 'api' is already in use")).toBe('name')
    expect(fieldForServerError("invalid cron schedule '* *': expected 5 fields")).toBe('schedule')
    expect(fieldForServerError("domain 'a.example.com' is already used by service 'web'")).toBe('domains')
    expect(fieldForServerError("env group 'nope' not found")).toBe('envGroups')
    expect(fieldForServerError("invalid repo_url 'x': must not be empty")).toBe('repoUrl')
    expect(fieldForServerError('something else entirely')).toBeNull()
  })
})
