import { describe, expect, it } from 'vitest'

import { isValidEnvKey, parseDotenv, toDotenv } from './dotenv'

describe('parseDotenv', () => {
  it('parses plain, exported, quoted and commented lines', () => {
    const { vars, invalid } = parseDotenv(
      [
        '# comment',
        'A=1',
        'export B = two',
        'C="quoted # not a comment"',
        "D='single $x'",
        'E=value # trailing comment',
        'F=',
        'G="line\\nbreak"',
        '',
      ].join('\n'),
    )
    expect(invalid).toEqual([])
    expect(vars).toEqual([
      { key: 'A', value: '1' },
      { key: 'B', value: 'two' },
      { key: 'C', value: 'quoted # not a comment' },
      { key: 'D', value: 'single $x' },
      { key: 'E', value: 'value' },
      { key: 'F', value: '' },
      { key: 'G', value: 'line\nbreak' },
    ])
  })

  it('supports multi-line double-quoted values and CRLF', () => {
    const { vars } = parseDotenv('KEY="-----BEGIN\r\nabc\r\n-----END"\r\nNEXT=1')
    expect(vars).toEqual([
      { key: 'KEY', value: '-----BEGIN\nabc\n-----END' },
      { key: 'NEXT', value: '1' },
    ])
  })

  it('reports invalid lines and keeps the last duplicate', () => {
    const { vars, invalid } = parseDotenv('1BAD=x\nnovalue\nA=1\nA=2')
    expect(invalid).toEqual(['1BAD=x', 'novalue'])
    expect(vars).toEqual([{ key: 'A', value: '2' }])
  })

  it('keeps references like ${{datastore.db.connectionString}} intact', () => {
    const { vars } = parseDotenv('DATABASE_URL=${{datastore.app-db.connectionString}}')
    expect(vars[0]?.value).toBe('${{datastore.app-db.connectionString}}')
  })
})

describe('toDotenv / isValidEnvKey', () => {
  it('round-trips values that need quoting', () => {
    const vars = [
      { key: 'A', value: 'simple' },
      { key: 'B', value: 'has space "and quotes"' },
      { key: 'C', value: 'multi\nline' },
    ]
    expect(parseDotenv(toDotenv(vars)).vars).toEqual(vars)
  })
  it('validates keys', () => {
    expect(isValidEnvKey('DATABASE_URL')).toBe(true)
    expect(isValidEnvKey('9LIVES')).toBe(false)
    expect(isValidEnvKey('has space')).toBe(false)
  })
})
