import { describe, expect, it } from 'vitest'

import { referencedResources, referencesInValue } from './env-refs'

describe('referencesInValue', () => {
  it('parses datastore and service references with aliases and spaces', () => {
    expect(referencesInValue('${{datastore.app-db.connectionString}}', 'DATABASE_URL')).toEqual([
      { kind: 'datastore', name: 'app-db', property: 'connectionString', key: 'DATABASE_URL' },
    ])
    expect(referencesInValue('x=${{ db.cache.host }}:${{svc.api.port}}/v1')).toEqual([
      { kind: 'datastore', name: 'cache', property: 'host', key: '' },
      { kind: 'service', name: 'api', property: 'port', key: '' },
    ])
  })

  it('skips escaped, malformed and unknown references', () => {
    expect(referencesInValue('$${{datastore.a.host}}')).toEqual([])
    expect(referencesInValue('${{datastore.a}}')).toEqual([])
    expect(referencesInValue('${{queue.a.host}}')).toEqual([])
    expect(referencesInValue('${{datastore.a.host')).toEqual([])
    expect(referencesInValue('plain value')).toEqual([])
  })
})

describe('referencedResources', () => {
  it('groups by resource and keeps every key once', () => {
    const res = referencedResources([
      { key: 'DATABASE_URL', value: '${{datastore.app-db.connectionString}}' },
      { key: 'DB_HOST', value: '${{database.app-db.host}}' },
      { key: 'API', value: 'http://${{service.api.hostport}} ${{service.api.hostport}}' },
      { key: 'PLAIN', value: 'hello' },
    ])
    expect(res).toEqual([
      { kind: 'datastore', name: 'app-db', keys: ['DATABASE_URL', 'DB_HOST'] },
      { kind: 'service', name: 'api', keys: ['API'] },
    ])
  })
})
