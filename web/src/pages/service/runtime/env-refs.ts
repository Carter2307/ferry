/**
 * Parse `${{ kind.name.property }}` references out of environment values
 * (mirror of `ferry-core/src/env.rs`): `datastore` (aliases `db`, `database`)
 * and `service` (alias `svc`). `$${{` is an escaped literal and is skipped.
 */

import type { EnvVar } from '@/lib/api/types'

export type RefKind = 'datastore' | 'service'

export interface EnvReference {
  kind: RefKind
  /** Referenced resource name. */
  name: string
  /** Property (`connectionString`, `hostport`…). */
  property: string
  /** Variable holding the reference. */
  key: string
}

/** One resource referenced by the env, with every variable pointing at it. */
export interface ReferencedResource {
  kind: RefKind
  name: string
  /** Variable names (deduplicated, in env order). */
  keys: string[]
}

const KIND_ALIASES: Record<string, RefKind> = {
  datastore: 'datastore',
  db: 'datastore',
  database: 'datastore',
  service: 'service',
  svc: 'service',
}

/** Every well-formed reference in one value (malformed ones are ignored). */
export function referencesInValue(value: string, key = ''): EnvReference[] {
  const out: EnvReference[] = []
  let i = 0
  while (i < value.length) {
    const start = value.indexOf('${{', i)
    if (start === -1) break
    if (start > 0 && value[start - 1] === '$') {
      // `$${{` = literal `${{`
      i = start + 3
      continue
    }
    const end = value.indexOf('}}', start + 3)
    if (end === -1) break
    const expr = value.slice(start + 3, end).trim()
    const parts = expr.split('.').map((p) => p.trim())
    if (parts.length === 3) {
      const [rawKind = '', name = '', property = ''] = parts
      const kind = KIND_ALIASES[rawKind.toLowerCase()]
      if (kind && name && property) out.push({ kind, name, property, key })
    }
    i = end + 2
  }
  return out
}

/** All references in a list of variables, grouped by resource (first-seen order). */
export function referencedResources(vars: readonly EnvVar[]): ReferencedResource[] {
  const byId = new Map<string, ReferencedResource>()
  for (const v of vars) {
    for (const ref of referencesInValue(v.value, v.key)) {
      const id = `${ref.kind}:${ref.name}`
      const existing = byId.get(id)
      if (existing) {
        if (!existing.keys.includes(v.key)) existing.keys.push(v.key)
      } else {
        byId.set(id, { kind: ref.kind, name: ref.name, keys: [v.key] })
      }
    }
  }
  return [...byId.values()]
}
