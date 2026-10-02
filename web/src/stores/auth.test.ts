import { beforeEach, describe, expect, it } from 'vitest'

import type { AuthStatus } from '@/lib/api/types'

import { useAuth } from './auth'

const user = { id: 'usr-1', email: 'ada@example.com', created_at: '2026-10-01T10:00:00Z' }
const status = (over: Partial<AuthStatus>): AuthStatus => ({ setup_required: false, auth: null, user: null, ...over })

describe('auth store', () => {
  beforeEach(() => useAuth.setState({ phase: 'loading', user: null, error: null }))

  it('follows what the server says', () => {
    useAuth.getState().apply(status({ setup_required: true }))
    expect(useAuth.getState().phase).toBe('setup')
    useAuth.getState().apply(status({}))
    expect(useAuth.getState().phase).toBe('signed-out')
    useAuth.getState().apply(status({ auth: 'session', user }))
    expect(useAuth.getState()).toMatchObject({ phase: 'signed-in', user })
  })

  it('is only signed in with a session', () => {
    // An API token sent along isn't the dashboard's session.
    useAuth.getState().apply(status({ auth: 'token' }))
    expect(useAuth.getState().phase).toBe('signed-out')
  })

  it('signs out only from a session', () => {
    useAuth.getState().apply(status({ auth: 'session', user }))
    useAuth.getState().signedOut()
    expect(useAuth.getState()).toMatchObject({ phase: 'signed-out', user: null })
    // A late 401 doesn't hide that the server needs its account first.
    useAuth.getState().apply(status({ setup_required: true }))
    useAuth.getState().signedOut()
    expect(useAuth.getState().phase).toBe('setup')
  })

  it('remembers why the server could not be asked', () => {
    useAuth.getState().unreachable('Could not reach the Ferry server.')
    expect(useAuth.getState()).toMatchObject({ phase: 'unreachable', error: 'Could not reach the Ferry server.' })
  })
})
