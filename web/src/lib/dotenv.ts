import type { EnvVar } from '@/lib/api/types'

const KEY_RE = /^[A-Za-z_][A-Za-z0-9_.-]*$/

/** True for a plausible environment variable name. */
export function isValidEnvKey(key: string): boolean {
  return KEY_RE.test(key)
}

/**
 * Parse `.env` text: `KEY=value`, optional `export ` prefix, `#` comments,
 * single/double quotes (double quotes expand \n, \t, \" and \\), inline
 * comments after unquoted values, and multi-line double-quoted values.
 * Later duplicates win. Invalid lines are reported, not thrown.
 */
export function parseDotenv(text: string): { vars: EnvVar[]; invalid: string[] } {
  const out = new Map<string, string>()
  const invalid: string[] = []
  const lines = text.replace(/\r\n?/g, '\n').split('\n')

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i] ?? ''
    const trimmed = raw.trim()
    if (!trimmed || trimmed.startsWith('#')) continue

    const body = trimmed.startsWith('export ') ? trimmed.slice(7).trimStart() : trimmed
    const eq = body.indexOf('=')
    if (eq <= 0) {
      invalid.push(raw)
      continue
    }
    const key = body.slice(0, eq).trim()
    if (!isValidEnvKey(key)) {
      invalid.push(raw)
      continue
    }
    let value = body.slice(eq + 1).trimStart()

    const quote = value[0]
    if (quote === '"' || quote === "'" || quote === '`') {
      let rest = value.slice(1)
      let closing = findClosingQuote(rest, quote)
      // multi-line quoted value
      while (closing === -1 && i + 1 < lines.length) {
        i++
        rest += '\n' + (lines[i] ?? '')
        closing = findClosingQuote(rest, quote)
      }
      if (closing === -1) {
        invalid.push(raw)
        continue
      }
      value = rest.slice(0, closing)
      if (quote === '"') value = unescapeDouble(value)
    } else {
      const hash = value.search(/\s#/)
      if (hash !== -1) value = value.slice(0, hash)
      value = value.trim()
    }
    out.set(key, value)
  }
  return { vars: [...out].map(([key, value]) => ({ key, value })), invalid }
}

function findClosingQuote(s: string, quote: string): number {
  for (let i = 0; i < s.length; i++) {
    if (s[i] === '\\' && quote === '"') {
      i++
      continue
    }
    if (s[i] === quote) return i
  }
  return -1
}

function unescapeDouble(s: string): string {
  return s.replace(/\\([nrt"\\$])/g, (_m, c: string) => {
    switch (c) {
      case 'n':
        return '\n'
      case 'r':
        return '\r'
      case 't':
        return '\t'
      default:
        return c
    }
  })
}

/** Serialize vars to `.env` text (values quoted when needed). */
export function toDotenv(vars: EnvVar[]): string {
  return vars
    .map(({ key, value }) => {
      if (/^[A-Za-z0-9_./:@%+,=-]*$/.test(value)) return `${key}=${value}`
      const escaped = value.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n')
      return `${key}="${escaped}"`
    })
    .join('\n')
}
