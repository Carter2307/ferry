/**
 * The browser's side of authorizing a GitHub / GitLab account.
 *
 * `POST /api/v1/git/authorize` answers with a page of the provider; the
 * browser goes there. The provider sends it back to `/git/callback`, which
 * hands the query parameters to `POST /api/v1/git/callback` and follows the
 * answer: the provider's next page (GitHub needs two: register the app, then
 * install it), or back to where the user was.
 *
 * Where the user was is kept in localStorage while the browser is away.
 */

import type { GitAuthorization, GitCallback } from '@/lib/api/types'
import { safeStorage } from '@/lib/storage'

/** The page providers send the browser back to (a route of this app). */
export const GIT_CALLBACK_PATH = '/git/callback'

const PENDING_KEY = 'ferry.git.authorizing'
/** An authorization left half-way is forgotten after this long (GitHub keeps its codes an hour). */
const PENDING_TTL_MS = 60 * 60 * 1000

/** `redirect_uri` of an authorization: this dashboard's callback page. */
export function gitRedirectUri(origin: string = window.location.origin): string {
  return `${origin}${GIT_CALLBACK_PATH}`
}

export interface PendingAuthorization {
  /** Where the user was (path + query of this app). */
  returnTo: string
  startedAt: number
}

/** A path of this app to go back to: never another origin. */
export function safeReturnTo(path: string | null | undefined, fallback = '/services'): string {
  if (!path || !path.startsWith('/') || path.startsWith('//') || path.startsWith('/\\')) return fallback
  if (path.startsWith(GIT_CALLBACK_PATH)) return fallback
  return path
}

export function savePending(returnTo: string, now: number = Date.now()): void {
  safeStorage.set(PENDING_KEY, JSON.stringify({ returnTo, startedAt: now }))
}

export function readPending(now: number = Date.now()): PendingAuthorization | null {
  const raw = safeStorage.get(PENDING_KEY)
  if (!raw) return null
  try {
    const v: unknown = JSON.parse(raw)
    if (typeof v !== 'object' || v === null) return null
    const o = v as Record<string, unknown>
    if (typeof o.returnTo !== 'string' || typeof o.startedAt !== 'number') return null
    if (now - o.startedAt > PENDING_TTL_MS) return null
    return { returnTo: safeReturnTo(o.returnTo), startedAt: o.startedAt }
  } catch {
    return null
  }
}

export function clearPending(): void {
  safeStorage.remove(PENDING_KEY)
}

/** The query parameters of the callback page, as `POST /api/v1/git/callback` takes them. */
export function callbackFromQuery(search: string): GitCallback {
  const q = new URLSearchParams(search)
  const text = (key: string) => q.get(key)?.trim() || undefined
  const installation = Number(q.get('installation_id') ?? '')
  return {
    state: q.get('state')?.trim() ?? '',
    code: text('code'),
    installation_id: Number.isSafeInteger(installation) && installation > 0 ? installation : undefined,
    setup_action: text('setup_action'),
    error: text('error'),
    error_description: text('error_description'),
  }
}

/** Whether the callback page was given anything to hand over. */
export function hasCallback(callback: GitCallback): boolean {
  return Boolean(callback.state || callback.installation_id)
}

/**
 * Send the browser to the provider page of a step: navigate, or post the
 * form (GitHub takes the app's manifest as a form field). `false` when the
 * step names no page.
 */
export function followAuthorization(step: GitAuthorization, target: Window = window): boolean {
  if (step.status !== 'redirect' || !step.url) return false
  if (step.method !== 'post') {
    target.location.assign(step.url)
    return true
  }
  const doc = target.document
  const form = doc.createElement('form')
  form.method = 'post'
  form.action = step.url
  form.style.display = 'none'
  for (const [name, value] of Object.entries(step.fields)) {
    const input = doc.createElement('input')
    input.type = 'hidden'
    input.name = name
    input.value = value
    form.appendChild(input)
  }
  doc.body.appendChild(form)
  form.submit()
  return true
}

/**
 * What the callback page tells the page it goes back to, in the router's
 * location state: an account was just connected.
 */
export interface GitConnectedState {
  gitConnected: string
}

export function gitConnectedState(connectionId: string): GitConnectedState {
  return { gitConnected: connectionId }
}

/** The id of the connection that was just made, from a location's state. */
export function connectedFromState(state: unknown): string | null {
  if (typeof state !== 'object' || state === null) return null
  const id = (state as Record<string, unknown>).gitConnected
  return typeof id === 'string' && id !== '' ? id : null
}
