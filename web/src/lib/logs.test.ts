import { describe, expect, it } from 'vitest'

import { logSeverity } from './logs'

describe('logSeverity', () => {
  it('keeps normal stderr progress neutral', () => {
    expect(logSeverity('stderr', '#5 [2/4] RUN npm ci')).toBeNull()
    expect(logSeverity('stderr', '#8 DONE 0.4s')).toBeNull()
    expect(logSeverity('stderr', '172.18.0.1 - - [26/Sep/2026:20:44:57 +0000] "GET / HTTP/1.1" 200 615')).toBeNull()
    expect(logSeverity('stderr', 'INFO:     Application startup complete.')).toBeNull()
  })

  it('flags error-level lines on any stream', () => {
    expect(logSeverity('stderr', '#9 ERROR: failed to solve: process "/bin/sh -c npm ci" did not complete')).toBe('error')
    expect(logSeverity('stderr', '2026/09/26 20:44:57 [error] 29#29: *1 open() failed')).toBe('error')
    expect(logSeverity('stdout', 'Traceback (most recent call last):')).toBe('error')
    expect(logSeverity('stdout', '{"level":"error","msg":"db down"}')).toBe('error')
    expect(logSeverity('stdout', 'time=… level=fatal msg=boom')).toBe('error')
    expect(logSeverity('stdout', "panic: runtime error: index out of range")).toBe('error')
    expect(logSeverity('stdout', 'ERROR: relation "users" does not exist')).toBe('error')
  })

  it('does not match words that merely contain a severity', () => {
    expect(logSeverity('stdout', 'compiled with 0 errors')).toBeNull()
    expect(logSeverity('stdout', 'error_log /var/log/nginx/error.log')).toBeNull()
    expect(logSeverity('stdout', 'terror')).toBeNull()
  })

  it('flags warnings', () => {
    expect(logSeverity('stderr', 'npm WARN deprecated inflight@1.0.6')).toBe('warning')
    expect(logSeverity('stdout', '[warning] disk almost full')).toBe('warning')
  })

  it('only marks failing system lines', () => {
    expect(logSeverity('system', '==> Starting deploy dep-1 (trigger: create)')).toBeNull()
    expect(logSeverity('system', '==> Deploy failed: instance d84d6c crashed (exit code 1)')).toBe('error')
  })
})
