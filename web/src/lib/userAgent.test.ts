import { describe, expect, it } from 'vitest'

import { describeUserAgent } from './userAgent'

describe('describeUserAgent', () => {
  it('names the browser and the system', () => {
    const cases: [string, string][] = [
      [
        'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36',
        'Chrome on macOS',
      ],
      [
        'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15',
        'Safari on macOS',
      ],
      ['Mozilla/5.0 (X11; Linux x86_64; rv:127.0) Gecko/20100101 Firefox/127.0', 'Firefox on Linux'],
      [
        'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0',
        'Edge on Windows',
      ],
      [
        'Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1',
        'Safari on iOS',
      ],
      [
        'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36',
        'Chrome on Android',
      ],
    ]
    for (const [ua, expected] of cases) expect(describeUserAgent(ua)).toBe(expected)
  })

  it('keeps what it does not know', () => {
    expect(describeUserAgent('curl/8.6.0')).toBe('curl')
    expect(describeUserAgent('my-script')).toBe('my-script')
    expect(describeUserAgent('x'.repeat(80))).toBe(`${'x'.repeat(60)}…`)
    expect(describeUserAgent(null)).toBe('Unknown browser')
    expect(describeUserAgent('  ')).toBe('Unknown browser')
  })
})
