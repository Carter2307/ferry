import { useAuth } from '@/stores/auth'

import type { ApiErrorBody } from './types'

/**
 * Error thrown for every failed API call. `message` is the server's
 * human-readable message (safe to show in toasts / error states).
 */
export class ApiError extends Error {
  readonly status: number
  readonly code: string

  constructor(status: number, code: string, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }

  get isNotFound(): boolean {
    return this.status === 404
  }
  get isConflict(): boolean {
    return this.status === 409
  }
  get isUnauthorized(): boolean {
    return this.status === 401
  }
}

/** Network failure (server unreachable, CORS, aborted by the browser…). */
export class NetworkError extends ApiError {
  constructor(message = 'Could not reach the Ferry server.') {
    super(0, 'network_error', message)
    this.name = 'NetworkError'
  }
}

export type QueryValue = string | number | boolean | null | undefined
export type Query = Record<string, QueryValue>

export interface RequestOptions {
  method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  query?: Query
  /** JSON-serialized unless it is a string/Blob (then sent as-is). */
  body?: unknown
  headers?: Record<string, string>
  signal?: AbortSignal
  /** Don't treat a 401 as the end of the session (asking whether there is one). */
  skipAuthRedirect?: boolean
}

/** API base: same origin (Vite proxies /api in dev; ferryd serves the SPA in prod). */
export const API_BASE = ''

function isApiErrorBody(v: unknown): v is ApiErrorBody {
  if (typeof v !== 'object' || v === null || !('error' in v)) return false
  const e = (v as { error: unknown }).error
  return typeof e === 'object' && e !== null && 'message' in e && typeof (e as { message: unknown }).message === 'string'
}

/** Build `/api/v1/...?a=b`, skipping null/undefined params. */
export function buildUrl(path: string, query?: Query): string {
  const url = `${API_BASE}${path}`
  if (!query) return url
  const params = new URLSearchParams()
  for (const [k, v] of Object.entries(query)) {
    if (v === undefined || v === null) continue
    params.set(k, String(v))
  }
  const qs = params.toString()
  return qs ? `${url}?${qs}` : url
}

/** Encode one path segment (`{id}` = id or name, domains…). */
export function seg(value: string): string {
  return encodeURIComponent(value)
}

/** Turn a non-2xx response into an ApiError (reads the JSON error body when present). */
export async function errorFromResponse(res: Response): Promise<ApiError> {
  let code = `http_${res.status}`
  let message = res.statusText || `Request failed with status ${res.status}`
  try {
    const text = await res.text()
    if (text) {
      try {
        const parsed: unknown = JSON.parse(text)
        if (isApiErrorBody(parsed)) {
          code = parsed.error.code || code
          message = parsed.error.message || message
        }
      } catch {
        if (text.length < 300) message = text.trim() || message
      }
    }
  } catch {
    /* body unreadable */
  }
  return new ApiError(res.status, code, message)
}

/**
 * The server no longer knows this browser's session (it expired, or was ended
 * elsewhere): back to the sign-in page. Other 401s, such as a wrong password,
 * say nothing about the session.
 */
export function handleUnauthorized(err: ApiError): void {
  if (err.status === 401 && err.code === 'unauthorized') useAuth.getState().signedOut()
}

/**
 * Perform an API request and decode the JSON response (`undefined` for 204).
 * The browser sends the session cookie itself (same origin).
 * Throws ApiError / NetworkError.
 */
export async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const { method = 'GET', query, body, signal, skipAuthRedirect } = opts
  const headers: Record<string, string> = { Accept: 'application/json', ...opts.headers }
  let payload: BodyInit | undefined
  if (body !== undefined) {
    if (typeof body === 'string' || body instanceof Blob) {
      payload = body
    } else {
      payload = JSON.stringify(body)
      headers['Content-Type'] = 'application/json'
    }
  }

  let res: Response
  try {
    res = await fetch(buildUrl(path, query), { method, headers, body: payload, signal, credentials: 'same-origin' })
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') throw e
    throw new NetworkError()
  }

  if (!res.ok) {
    const err = await errorFromResponse(res)
    if (!skipAuthRedirect) handleUnauthorized(err)
    throw err
  }
  if (res.status === 204 || res.status === 205) return undefined as T
  const text = await res.text()
  if (!text) return undefined as T
  const type = res.headers.get('content-type') ?? ''
  if (!type.includes('json')) return text as T
  return JSON.parse(text) as T
}

/** Thin verb helpers. */
export const api = {
  get: <T>(path: string, query?: Query, signal?: AbortSignal) => request<T>(path, { query, signal }),
  post: <T>(path: string, body?: unknown, query?: Query) => request<T>(path, { method: 'POST', body, query }),
  put: <T>(path: string, body?: unknown, query?: Query) => request<T>(path, { method: 'PUT', body, query }),
  patch: <T>(path: string, body?: unknown, query?: Query) => request<T>(path, { method: 'PATCH', body, query }),
  delete: <T = void>(path: string, query?: Query) => request<T>(path, { method: 'DELETE', query }),
}

/** A readable message for any thrown value (for toasts / error states). */
export function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message
  if (typeof err === 'string') return err
  return 'Something went wrong.'
}
