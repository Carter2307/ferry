/**
 * Server-Sent Events over `fetch()` (so the API token travels in the
 * `Authorization` header, never in the URL).
 *
 * `SseParser` implements the WHATWG event-stream interpretation rules:
 * LF / CR / CRLF line endings (also split across chunks), `:` comments,
 * `event` / `data` / `id` / `retry` fields, multi-line `data`, an optional
 * leading BOM, and dispatch on blank lines. `readSse` drives it from a
 * streaming response body.
 */

import { authHeaders, buildUrl, errorFromResponse, handleUnauthorized, NetworkError, type Query } from './client'

export interface SseEvent {
  /** `event:` field, `message` when absent. */
  event: string
  /** `data:` lines joined with `\n`. */
  data: string
  /** Last event id seen on the stream (`id:` field). */
  id: string
}

export class SseParser {
  private buffer = ''
  private skipLeadingLf = false
  private bomChecked = false
  private dataLines: string[] | null = null
  private eventType = ''
  private lastEventId = ''
  /** Reconnection time requested by the server (`retry:`), in ms. */
  retry: number | null = null

  /** Feed a decoded chunk; returns the events completed by it. */
  push(chunk: string): SseEvent[] {
    let text = chunk
    if (!this.bomChecked && text.length > 0) {
      this.bomChecked = true
      if (text.charCodeAt(0) === 0xfeff) text = text.slice(1)
    }
    if (this.skipLeadingLf && text.length > 0) {
      this.skipLeadingLf = false
      if (text[0] === '\n') text = text.slice(1)
    }
    this.buffer += text

    const events: SseEvent[] = []
    let start = 0
    for (let i = 0; i < this.buffer.length; i++) {
      const ch = this.buffer[i]
      if (ch !== '\n' && ch !== '\r') continue
      const line = this.buffer.slice(start, i)
      if (ch === '\r') {
        if (i + 1 < this.buffer.length) {
          if (this.buffer[i + 1] === '\n') i++
        } else {
          // CR at the very end of the chunk: a following LF belongs to it.
          this.skipLeadingLf = true
        }
      }
      start = i + 1
      const ev = this.processLine(line)
      if (ev) events.push(ev)
    }
    this.buffer = this.buffer.slice(start)
    return events
  }

  /** End of stream: an unterminated event is discarded (per spec). */
  reset(): void {
    this.buffer = ''
    this.dataLines = null
    this.eventType = ''
    this.skipLeadingLf = false
  }

  private processLine(line: string): SseEvent | null {
    if (line === '') return this.dispatch()
    if (line.startsWith(':')) return null // comment / keep-alive

    const colon = line.indexOf(':')
    let field: string
    let value: string
    if (colon === -1) {
      field = line
      value = ''
    } else {
      field = line.slice(0, colon)
      value = line.slice(colon + 1)
      if (value.startsWith(' ')) value = value.slice(1)
    }

    switch (field) {
      case 'event':
        this.eventType = value
        break
      case 'data':
        ;(this.dataLines ??= []).push(value)
        break
      case 'id':
        if (!value.includes('\0')) this.lastEventId = value
        break
      case 'retry':
        if (/^\d+$/.test(value)) this.retry = Number(value)
        break
      default:
        break // unknown fields are ignored
    }
    return null
  }

  private dispatch(): SseEvent | null {
    const lines = this.dataLines
    const type = this.eventType
    this.dataLines = null
    this.eventType = ''
    // No data field at all → nothing to dispatch (spec: empty data buffer).
    if (lines === null) return null
    return { event: type || 'message', data: lines.join('\n'), id: this.lastEventId }
  }
}

export interface ReadSseOptions {
  query?: Query
  signal: AbortSignal
  /** Called once the response headers arrived with a 2xx status. */
  onOpen?: () => void
  onEvent: (event: SseEvent) => void
}

/**
 * Open an SSE stream with `fetch` and dispatch its events until the server
 * closes it (resolves) or `signal` aborts (rejects with AbortError).
 * Non-2xx responses reject with ApiError (401 signs out).
 * Resolves with the server-requested retry delay, if any.
 */
export async function readSse(path: string, opts: ReadSseOptions): Promise<{ retry: number | null }> {
  let res: Response
  try {
    res = await fetch(buildUrl(path, opts.query), {
      headers: { Accept: 'text/event-stream', 'Cache-Control': 'no-cache', ...authHeaders() },
      signal: opts.signal,
      credentials: 'same-origin',
      cache: 'no-store',
    })
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') throw e
    throw new NetworkError()
  }
  if (!res.ok) {
    const err = await errorFromResponse(res)
    if (res.status === 401) handleUnauthorized(true)
    throw err
  }
  if (!res.body) throw new NetworkError('The server returned an empty stream.')
  opts.onOpen?.()

  const parser = new SseParser()
  const reader = res.body.getReader()
  const decoder = new TextDecoder('utf-8')
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      const chunk = decoder.decode(value, { stream: true })
      for (const ev of parser.push(chunk)) opts.onEvent(ev)
    }
    const tail = decoder.decode()
    if (tail) for (const ev of parser.push(tail)) opts.onEvent(ev)
  } catch (e) {
    if (opts.signal.aborted) throw new DOMException('Aborted', 'AbortError')
    throw e instanceof Error ? e : new NetworkError()
  } finally {
    reader.releaseLock()
  }
  return { retry: parser.retry }
}

/** Exponential backoff: 1s, 2s, 4s, 8s, 15s, 15s… */
export function backoffDelay(attempt: number, minMs = 1000, maxMs = 15000): number {
  return Math.min(maxMs, minMs * 2 ** Math.max(0, attempt))
}

/** Promise that resolves after `ms`, or rejects with AbortError when `signal` aborts. */
export function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(new DOMException('Aborted', 'AbortError'))
      return
    }
    const t = setTimeout(() => {
      signal.removeEventListener('abort', onAbort)
      resolve()
    }, ms)
    const onAbort = () => {
      clearTimeout(t)
      reject(new DOMException('Aborted', 'AbortError'))
    }
    signal.addEventListener('abort', onAbort, { once: true })
  })
}

export function isAbortError(e: unknown): boolean {
  return e instanceof DOMException && e.name === 'AbortError'
}
