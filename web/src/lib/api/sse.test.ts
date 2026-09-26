import { describe, expect, it } from 'vitest'

import { backoffDelay, SseParser } from './sse'

function parseAll(chunks: string[]) {
  const p = new SseParser()
  return chunks.flatMap((c) => p.push(c))
}

describe('SseParser', () => {
  it('parses a log event and the empty-data end event (ferry framing)', () => {
    const evs = parseAll(['event: log\ndata: {"line":"hi"}\n\nevent: end\ndata: \n\n'])
    expect(evs).toEqual([
      { event: 'log', data: '{"line":"hi"}', id: '' },
      { event: 'end', data: '', id: '' },
    ])
  })

  it('defaults the event type to "message"', () => {
    expect(parseAll(['data: x\n\n'])).toEqual([{ event: 'message', data: 'x', id: '' }])
  })

  it('handles CRLF and lone CR line endings', () => {
    expect(parseAll(['event: a\r\ndata: 1\r\n\r\n'])).toEqual([{ event: 'a', data: '1', id: '' }])
    expect(parseAll(['event: b\rdata: 2\r\r'])).toEqual([{ event: 'b', data: '2', id: '' }])
  })

  it('does not double count a CRLF split across chunks', () => {
    const evs = parseAll(['data: a\r', '\n\r', '\ndata: b\r\n\r\n'])
    expect(evs.map((e) => e.data)).toEqual(['a', 'b'])
  })

  it('joins multi-line data with \\n', () => {
    expect(parseAll(['data: one\ndata: two\ndata:three\n\n'])[0]?.data).toBe('one\ntwo\nthree')
  })

  it('ignores comments / keep-alives and unknown fields', () => {
    const evs = parseAll([': keep-alive\n\n', 'foo: bar\n', ':x\ndata: y\n\n'])
    expect(evs).toEqual([{ event: 'message', data: 'y', id: '' }])
  })

  it('reassembles events split at arbitrary points', () => {
    const src = 'event: change\ndata: {"kind":"service","id":"srv-1"}\n\nevent: ready\ndata: \n\n'
    for (let size = 1; size <= 7; size++) {
      const chunks: string[] = []
      for (let i = 0; i < src.length; i += size) chunks.push(src.slice(i, i + size))
      const evs = parseAll(chunks)
      expect(evs.map((e) => e.event)).toEqual(['change', 'ready'])
      expect(evs[0]?.data).toBe('{"kind":"service","id":"srv-1"}')
    }
  })

  it('keeps an unterminated event pending until the blank line arrives', () => {
    const p = new SseParser()
    expect(p.push('event: log\ndata: partial')).toEqual([])
    expect(p.push('\n')).toEqual([])
    expect(p.push('\n')).toEqual([{ event: 'log', data: 'partial', id: '' }])
  })

  it('does not dispatch events without a data field', () => {
    expect(parseAll(['event: nothing\n\n'])).toEqual([])
  })

  it('tracks id and retry, strips only one leading space and a BOM', () => {
    const p = new SseParser()
    const evs = p.push('﻿id: 42\nretry: 3000\ndata:  two spaces\n\n')
    expect(evs).toEqual([{ event: 'message', data: ' two spaces', id: '42' }])
    expect(p.retry).toBe(3000)
    // id persists for later events
    expect(p.push('data: next\n\n')[0]?.id).toBe('42')
  })

  it('treats a field without colon as an empty value', () => {
    expect(parseAll(['data\n\n'])).toEqual([{ event: 'message', data: '', id: '' }])
  })
})

describe('backoffDelay', () => {
  it('grows exponentially from 1s and caps at 15s', () => {
    expect([0, 1, 2, 3, 4, 5, 10].map((a) => backoffDelay(a))).toEqual([1000, 2000, 4000, 8000, 15000, 15000, 15000])
  })
})
