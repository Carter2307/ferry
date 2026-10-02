// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { GitAuthorization } from '@/lib/api/types'

import {
  callbackFromQuery,
  clearPending,
  connectedFromState,
  followAuthorization,
  gitConnectedState,
  gitRedirectUri,
  hasCallback,
  readPending,
  safeReturnTo,
  savePending,
} from './gitAuthorize'

afterEach(() => {
  clearPending()
  document.body.innerHTML = ''
})

describe('gitRedirectUri', () => {
  it('is the callback page of this dashboard', () => {
    expect(gitRedirectUri('https://ferry.example.com')).toBe('https://ferry.example.com/git/callback')
    expect(gitRedirectUri('http://localhost:7878')).toBe('http://localhost:7878/git/callback')
  })
})

describe('callbackFromQuery', () => {
  it('reads what the providers send back', () => {
    // GitHub, after the app was registered; GitLab, after the account authorized.
    expect(callbackFromQuery('?code=abc123&state=st4te')).toEqual({ state: 'st4te', code: 'abc123' })
    // GitHub, after the app was installed.
    expect(callbackFromQuery('?installation_id=77&setup_action=install&state=s2')).toEqual({
      state: 's2',
      installation_id: 77,
      setup_action: 'install',
    })
    // GitHub, after an installation was changed on its own pages: no state.
    expect(callbackFromQuery('?installation_id=77&setup_action=update')).toEqual({
      state: '',
      installation_id: 77,
      setup_action: 'update',
    })
    // The user refused.
    expect(callbackFromQuery('?error=access_denied&error_description=The+user+denied&state=s3')).toEqual({
      state: 's3',
      error: 'access_denied',
      error_description: 'The user denied',
    })
  })

  it('ignores what is not an installation id', () => {
    for (const bad of ['abc', '-1', '0', '1.5', '', '99999999999999999999']) {
      expect(callbackFromQuery(`?installation_id=${bad}&state=s`).installation_id, bad).toBeUndefined()
    }
  })

  it('knows when there is nothing to hand over', () => {
    expect(hasCallback(callbackFromQuery(''))).toBe(false)
    expect(hasCallback(callbackFromQuery('?code=abc'))).toBe(false)
    expect(hasCallback(callbackFromQuery('?state=s'))).toBe(true)
    expect(hasCallback(callbackFromQuery('?installation_id=5'))).toBe(true)
  })
})

describe('where to go back', () => {
  it('only ever goes back to a path of this app', () => {
    expect(safeReturnTo('/services/new')).toBe('/services/new')
    expect(safeReturnTo('/server?section=connections')).toBe('/server?section=connections')
    for (const bad of [null, undefined, '', 'https://evil.example', '//evil.example', '/\\evil.example', 'services']) {
      expect(safeReturnTo(bad), String(bad)).toBe('/services')
    }
    // Never back to the callback page itself.
    expect(safeReturnTo('/git/callback?code=x')).toBe('/services')
    expect(safeReturnTo(null, '/server')).toBe('/server')
  })

  it('remembers where an authorization started, for an hour', () => {
    expect(readPending()).toBeNull()
    const now = Date.parse('2026-10-01T12:00:00Z')
    savePending('/services/new', now)
    expect(readPending(now + 60_000)).toEqual({ returnTo: '/services/new', startedAt: now })
    expect(readPending(now + 61 * 60_000)).toBeNull()
    // What was stored is not trusted.
    savePending('https://evil.example', now)
    expect(readPending(now)?.returnTo).toBe('/services')
    window.localStorage.setItem('ferry.git.authorizing', '{not json')
    expect(readPending(now)).toBeNull()
    clearPending()
    expect(readPending(now)).toBeNull()
  })

  it('tells the page it goes back to which account was connected', () => {
    expect(connectedFromState(gitConnectedState('git-0123456789abcdef0123'))).toBe('git-0123456789abcdef0123')
    for (const other of [null, undefined, 'x', {}, { gitConnected: '' }, { gitConnected: 5 }]) {
      expect(connectedFromState(other)).toBeNull()
    }
  })
})

describe('followAuthorization', () => {
  const step = (over: Partial<GitAuthorization>): GitAuthorization => ({
    status: 'redirect',
    url: 'https://github.com/settings/apps/new?state=abc',
    method: 'get',
    fields: {},
    connection: null,
    ...over,
  })

  /** A window whose navigations are recorded instead of happening. */
  function target() {
    const assign = vi.fn()
    const win = { location: { assign }, document } as unknown as Window
    return { win, assign }
  }

  it('navigates to the provider page', () => {
    const { win, assign } = target()
    expect(followAuthorization(step({ url: 'https://gitlab.com/oauth/authorize?client_id=x' }), win)).toBe(true)
    expect(assign).toHaveBeenCalledWith('https://gitlab.com/oauth/authorize?client_id=x')
  })

  it('posts the form GitHub takes the manifest in', () => {
    const { win, assign } = target()
    const submit = vi.spyOn(HTMLFormElement.prototype, 'submit').mockImplementation(() => {})
    const manifest = JSON.stringify({ name: 'ferry-localhost-ab12cd', public: false })
    expect(followAuthorization(step({ method: 'post', fields: { manifest } }), win)).toBe(true)
    const form = document.querySelector('form')
    expect(form?.method).toBe('post')
    expect(form?.action).toBe('https://github.com/settings/apps/new?state=abc')
    const input = form?.querySelector<HTMLInputElement>('input[name="manifest"]')
    expect([input?.type, input?.value]).toEqual(['hidden', manifest])
    expect(submit).toHaveBeenCalledTimes(1)
    expect(assign).not.toHaveBeenCalled()
    submit.mockRestore()
  })

  it('does nothing when there is no page to go to', () => {
    const { win, assign } = target()
    expect(followAuthorization(step({ status: 'connected', url: null, method: null }), win)).toBe(false)
    expect(followAuthorization(step({ url: null }), win)).toBe(false)
    expect(assign).not.toHaveBeenCalled()
  })
})
