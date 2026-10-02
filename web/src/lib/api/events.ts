/**
 * Live updates: subscribe to the `/api/v1/events` change feed (SSE) and turn
 * each change into TanStack Query invalidations. When the server has no
 * change feed (404 — older ferryd) or while the stream is down, queries fall
 * back to polling via `usePollInterval`.
 */

import { useEffect } from 'react'
import { useQueryClient, type QueryClient, type QueryKey } from '@tanstack/react-query'
import { create } from 'zustand'

import { useSignedIn } from '@/stores/auth'

import { ApiError } from './client'
import { streamPaths } from './endpoints'
import { keys } from './keys'
import { backoffDelay, isAbortError, readSse, sleep } from './sse'
import type { ChangeEvent, ChangeKind } from './types'

/**
 * - `connecting`: first attempt in flight (poll meanwhile)
 * - `live`: change feed connected, no polling needed
 * - `reconnecting`: feed dropped, retrying with backoff (poll meanwhile)
 * - `polling`: server has no change feed; poll for the whole session
 * - `off`: signed out
 */
export type LiveMode = 'off' | 'connecting' | 'live' | 'reconnecting' | 'polling'

export const useLive = create<{ mode: LiveMode }>()(() => ({ mode: 'off' }))

function setMode(mode: LiveMode): void {
  if (useLive.getState().mode !== mode) useLive.setState({ mode })
}

/**
 * `refetchInterval` for a query: `ms` unless the change feed is live.
 * Pass `always: true` for data the feed doesn't cover (runtime metrics).
 */
export function usePollInterval(ms: number, opts: { always?: boolean } = {}): number | false {
  const mode = useLive((s) => s.mode)
  if (opts.always) return ms
  return mode === 'live' ? false : ms
}

const CHANGE_KINDS: readonly ChangeKind[] = [
  'service',
  'deploy',
  'datastore',
  'env_group',
  'job',
  'git_connection',
  'domain',
]

function isChangeEvent(v: unknown): v is ChangeEvent {
  if (typeof v !== 'object' || v === null) return false
  const o = v as Record<string, unknown>
  return typeof o.kind === 'string' && (CHANGE_KINDS as readonly string[]).includes(o.kind) && typeof o.id === 'string'
}

/** One invalidation target: a key prefix, or every service's jobs list. */
export type Invalidation = QueryKey | 'service-jobs'

/** What to invalidate for one change. */
export function invalidationsForChange(change: ChangeEvent): Invalidation[] {
  switch (change.kind) {
    case 'service':
      // list + every service subtree (details are cached by name, events carry ids)
      return [keys.services()]
    case 'deploy':
      // latest_deploy / state live on the service views
      return [keys.deploy(change.id), keys.services()]
    case 'job':
      return [keys.job(change.id), 'service-jobs']
    case 'datastore':
      return [keys.datastores()]
    case 'env_group':
      return change.action === 'deleted' ? [keys.envGroups(), keys.services()] : [keys.envGroups()]
    case 'git_connection':
      // the accounts, and what they let the server read (repositories, branches)
      return [keys.git()]
    case 'domain':
      // the domains and their certificates, the default domain of /info, and
      // the hosts and URL of every service
      return [keys.domains(), keys.info(), keys.services()]
  }
}

function invalidate(qc: QueryClient, batch: Invalidation[]): void {
  const seen = new Set<string>()
  for (const item of batch) {
    const id = JSON.stringify(item)
    if (seen.has(id)) continue
    seen.add(id)
    if (item === 'service-jobs') {
      void qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === 'services' && q.queryKey[3] === 'jobs' })
    } else {
      void qc.invalidateQueries({ queryKey: item })
    }
  }
}

/**
 * Mount once (AppShell). Keeps a change-feed subscription open while signed
 * in, reconnecting with exponential backoff (1s → 15s), and stops on sign-out.
 */
export function useChangeFeed(): void {
  const qc = useQueryClient()
  const signedIn = useSignedIn()

  useEffect(() => {
    if (!signedIn) {
      setMode('off')
      return
    }
    const ctrl = new AbortController()
    const { signal } = ctrl
    let pending: Invalidation[] = []
    let flushTimer: ReturnType<typeof setTimeout> | null = null

    const queue = (batch: Invalidation[]) => {
      pending.push(...batch)
      if (flushTimer) return
      flushTimer = setTimeout(() => {
        flushTimer = null
        const b = pending
        pending = []
        invalidate(qc, b)
      }, 150)
    }

    void (async () => {
      let attempt = 0
      let connectedBefore = false
      setMode('connecting')
      while (!signal.aborted) {
        try {
          await readSse(streamPaths.events, {
            signal,
            onOpen: () => {
              attempt = 0
            },
            onEvent: (ev) => {
              if (ev.event === 'ready') {
                setMode('live')
                // Anything may have changed while we were disconnected.
                if (connectedBefore) void qc.invalidateQueries()
                connectedBefore = true
              } else if (ev.event === 'change') {
                try {
                  const parsed: unknown = JSON.parse(ev.data)
                  if (isChangeEvent(parsed)) queue(invalidationsForChange(parsed))
                } catch {
                  /* malformed event: ignore */
                }
              }
            },
          })
          // Server closed the stream (restart / shutdown): reconnect.
        } catch (e) {
          if (isAbortError(e) || signal.aborted) return
          if (e instanceof ApiError && (e.status === 404 || e.status === 405)) {
            setMode('polling') // older server without a change feed
            return
          }
          if (e instanceof ApiError && e.status === 401) return // signed out by the client
        }
        if (signal.aborted) return
        setMode('reconnecting')
        try {
          await sleep(backoffDelay(attempt++), signal)
        } catch {
          return
        }
      }
    })()

    return () => {
      ctrl.abort()
      if (flushTimer) clearTimeout(flushTimer)
    }
  }, [qc, signedIn])
}
