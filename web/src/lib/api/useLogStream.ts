import { useCallback, useEffect, useRef, useState } from 'react'

import { stripAnsi } from '@/lib/format'
import { useAuth } from '@/stores/auth'

import { ApiError, errorMessage } from './client'
import { backoffDelay, isAbortError, readSse, sleep } from './sse'
import type { LogLine } from './types'

export type LogStreamStatus = 'idle' | 'connecting' | 'live' | 'ended' | 'error'

/** A received line plus a stable sequence number and its ANSI-free text. */
export interface LogEntry extends LogLine {
  seq: number
  /** `line` without ANSI escape codes (what the viewer renders / searches). */
  text: string
}

export interface LogStreamOptions {
  /** Keep the stream open for new lines (`?follow=true`). Default true. */
  follow?: boolean
  /** Runtime logs only: number of past lines per instance (`?tail=`). */
  tail?: number
  /** Set false to keep the stream closed. Default true. */
  enabled?: boolean
  /** Ring-buffer size; older lines are dropped. Default 5000. */
  maxLines?: number
  /** Reconnect after network drops while following. Default true. */
  reconnect?: boolean
}

export interface LogStreamResult {
  lines: LogEntry[]
  status: LogStreamStatus
  /** Error message when status is `error` (or the last reconnect reason). */
  error: string | null
  /** Drop the lines received so far (the stream stays open). */
  clear: () => void
  /** Close and reopen the stream from scratch. */
  restart: () => void
}

/**
 * Stream an SSE log endpoint (`event: log` lines, `event: end` terminator).
 *
 * `path` e.g. `streamPaths.deployLogs(id)`; pass null to stay idle.
 * Lines are batched per animation frame so fast builds don't re-render per
 * line. On a network drop while following it reconnects with backoff:
 * finite logs (deploy/job) are replayed from scratch, runtime logs (with
 * `tail`) resume with `tail=0` so nothing is duplicated.
 */
export function useLogStream(path: string | null, options: LogStreamOptions = {}): LogStreamResult {
  const { follow = true, tail, enabled = true, maxLines = 5000, reconnect = true } = options
  const token = useAuth((s) => s.token)

  const [lines, setLines] = useState<LogEntry[]>([])
  const [status, setStatus] = useState<LogStreamStatus>('idle')
  const [error, setError] = useState<string | null>(null)
  const [generation, setGeneration] = useState(0)

  const seqRef = useRef(0)
  const pendingRef = useRef<LogEntry[]>([])
  const frameRef = useRef<number | null>(null)
  const maxRef = useRef(maxLines)
  useEffect(() => {
    maxRef.current = maxLines
  }, [maxLines])

  // Reset the buffer when the stream identity changes (render-time reset, no effect).
  const active = Boolean(path && enabled && token)
  const streamKey = active ? `${path}|${follow}|${tail ?? ''}|${generation}` : null
  const [currentKey, setCurrentKey] = useState<string | null>(null)
  if (streamKey !== currentKey) {
    setCurrentKey(streamKey)
    setLines([])
    setError(null)
    setStatus(streamKey ? 'connecting' : 'idle')
  }

  const flush = useCallback(() => {
    frameRef.current = null
    const batch = pendingRef.current
    if (batch.length === 0) return
    pendingRef.current = []
    setLines((prev) => {
      const next = prev.length + batch.length > maxRef.current ? [...prev, ...batch].slice(-maxRef.current) : [...prev, ...batch]
      return next
    })
  }, [])

  const scheduleFlush = useCallback(() => {
    if (frameRef.current !== null) return
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') {
      frameRef.current = window.setTimeout(flush, 250) as unknown as number
    } else {
      frameRef.current = window.requestAnimationFrame(flush)
    }
  }, [flush])

  const clear = useCallback(() => {
    pendingRef.current = []
    setLines([])
  }, [])

  const restart = useCallback(() => setGeneration((g) => g + 1), [])

  useEffect(() => {
    if (!path || !enabled || !token) return
    const ctrl = new AbortController()
    const { signal } = ctrl
    const resumable = tail !== undefined
    pendingRef.current = []

    const push = (raw: string) => {
      try {
        const line = JSON.parse(raw) as LogLine
        if (typeof line.line !== 'string') return
        pendingRef.current.push({ ...line, seq: ++seqRef.current, text: stripAnsi(line.line) })
        scheduleFlush()
      } catch {
        /* malformed line: skip */
      }
    }

    void (async () => {
      let attempt = 0
      let firstConnect = true
      while (!signal.aborted) {
        let ended = false
        try {
          const query: Record<string, string | number | boolean> = { follow }
          if (tail !== undefined) query.tail = firstConnect ? tail : 0
          if (!firstConnect && !resumable) {
            // finite log replayed from the start: drop what we had
            pendingRef.current = []
            setLines([])
          }
          await readSse(path, {
            query,
            signal,
            onOpen: () => {
              attempt = 0
              setError(null)
              setStatus('live')
            },
            onEvent: (ev) => {
              if (ev.event === 'log') push(ev.data)
              else if (ev.event === 'end') ended = true
            },
          })
          if (ended || !follow) {
            flush()
            setStatus('ended')
            return
          }
          // closed without `end` (server restart): reconnect below
        } catch (e) {
          if (isAbortError(e) || signal.aborted) return
          if (e instanceof ApiError && e.status > 0 && e.status < 500) {
            flush()
            setError(e.message)
            setStatus('error')
            return
          }
          setError(errorMessage(e))
        }
        flush()
        if (!reconnect || !follow) {
          setStatus('error')
          return
        }
        firstConnect = false
        setStatus('connecting')
        try {
          await sleep(backoffDelay(attempt++), signal)
        } catch {
          return
        }
      }
    })()

    return () => {
      ctrl.abort()
      if (frameRef.current !== null) {
        window.cancelAnimationFrame(frameRef.current)
        window.clearTimeout(frameRef.current)
        frameRef.current = null
      }
    }
  }, [path, follow, tail, enabled, token, reconnect, generation, flush, scheduleFlush])

  return { lines, status, error, clear, restart }
}
