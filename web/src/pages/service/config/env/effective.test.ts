import { describe, expect, it } from 'vitest'

import { findReferences, mergeLayers, suggestedKey, type EnvLayer } from './effective'

const layer = (id: string, vars: Record<string, string>): EnvLayer => ({
  id,
  kind: id === 'service' ? 'service' : 'group',
  label: id,
  vars: Object.entries(vars).map(([key, value]) => ({ key, value })),
})

describe('mergeLayers', () => {
  it('lets later layers win and keeps first-appearance order (env::merge)', () => {
    const g1 = layer('g1', { A: 'g1', B: 'g1' })
    const g2 = layer('g2', { B: 'g2' })
    const svc = layer('service', { C: 'svc', A: 'svc' })
    const out = mergeLayers([g1, g2, svc])
    expect(out.map((v) => [v.key, v.value, v.source.id])).toEqual([
      ['A', 'svc', 'service'],
      ['B', 'g2', 'g2'],
      ['C', 'svc', 'service'],
    ])
    expect(out[0]?.overridden.map((l) => l.id)).toEqual(['g1'])
    expect(out[1]?.overridden.map((l) => l.id)).toEqual(['g1'])
    expect(out[2]?.overridden).toEqual([])
  })

  it('records every overridden layer in order', () => {
    const out = mergeLayers([layer('a', { K: '1' }), layer('b', { K: '2' }), layer('service', { K: '3' })])
    expect(out).toHaveLength(1)
    expect(out[0]?.value).toBe('3')
    expect(out[0]?.overridden.map((l) => l.id)).toEqual(['a', 'b'])
  })
})

describe('findReferences', () => {
  const known = { datastores: ['app-db'], services: ['api'] }

  it('parses datastore and service references (with aliases and spaces)', () => {
    const refs = findReferences('x ${{ datastore.app-db.connectionString }} y ${{svc.api.hostport}}', known)
    expect(refs.map((r) => [r.kind, r.name, r.property, r.problem])).toEqual([
      ['datastore', 'app-db', 'connectionString', null],
      ['service', 'api', 'hostport', null],
    ])
  })

  it('skips $${{ escapes', () => {
    expect(findReferences('tpl $${{ x }} and ${{db.app-db.port}}', known)).toHaveLength(1)
    expect(findReferences('$${{ literal }}', known)).toEqual([])
  })

  it('reports unknown names, properties, kinds and syntax errors', () => {
    const problems = (v: string) => findReferences(v, known).map((r) => r.problem)
    expect(problems('${{datastore.nope.connectionString}}')[0]).toMatch(/Unknown datastore/)
    expect(problems('${{service.api.nope}}')[0]).toMatch(/Unknown service property/)
    expect(problems('${{queue.a.b}}')[0]).toMatch(/Unknown reference kind/)
    expect(problems('${{datastore.app-db}}')[0]).toMatch(/Expected/)
    expect(problems('${{datastore.app-db.port')[0]).toMatch(/Unterminated/)
  })

  it('skips the name check when the lists are not loaded', () => {
    expect(findReferences('${{datastore.whatever.host}}', null)[0]?.problem).toBeNull()
  })
})

describe('suggestedKey', () => {
  it('prefers the conventional name, then one derived from the datastore', () => {
    expect(suggestedKey('postgres', 'app-db', [])).toBe('DATABASE_URL')
    expect(suggestedKey('redis', 'cache', [])).toBe('REDIS_URL')
    expect(suggestedKey('postgres', 'app-db', ['DATABASE_URL'])).toBe('APP_DB_URL')
    expect(suggestedKey('postgres', 'app-db', ['DATABASE_URL', 'APP_DB_URL'])).toBe('APP_DB_URL_2')
  })
})
