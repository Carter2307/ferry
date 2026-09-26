/**
 * Pure helpers for the datastore pages: name / identifier validation (mirrors
 * `validate::resource_name` and `checks::pg_identifier`), `${{datastore.…}}`
 * references, and connection-string masking.
 */

import type { DatastoreKind } from '@/lib/api/types'

/** Majors offered in the create dialog (server default first). */
export const DATASTORE_VERSIONS: Record<DatastoreKind, { default: string; options: string[] }> = {
  postgres: { default: '16', options: ['18', '17', '16', '15', '14', '13'] },
  redis: { default: '7', options: ['8', '7', '6'] },
}

export const DATASTORE_KIND_DESCRIPTIONS: Record<DatastoreKind, string> = {
  postgres: 'Relational database with a dedicated user and database.',
  redis: 'In-memory key-value store with append-only persistence.',
}

/** Conventional env var name for a datastore's connection string. */
export function suggestedEnvKey(kind: DatastoreKind): string {
  return kind === 'postgres' ? 'DATABASE_URL' : 'REDIS_URL'
}

const RESERVED_NAMES = new Set(['ferry', 'localhost'])
const ID_PREFIXES = ['srv', 'dep', 'job', 'dbs', 'evg']

/** Same shape check as `validate::looks_like_id` (`dbs-1a2b…`). */
function looksLikeId(name: string): boolean {
  return ID_PREFIXES.some((p) => name.startsWith(`${p}-`) && /^[0-9a-f]*$/.test(name.slice(p.length + 1)))
}

/**
 * Service / datastore name (they share one namespace and double as private
 * hostnames). Returns an error message, or null when valid.
 */
export function resourceNameError(name: string, taken: ReadonlySet<string> = new Set()): string | null {
  if (name === '') return 'Name is required'
  if (name.length > 40) return 'Use at most 40 characters'
  if (!/^[a-z]/.test(name)) return 'Start with a lowercase letter'
  if (!/^[a-z0-9-]+$/.test(name)) return 'Use lowercase letters, digits and “-” only'
  if (name.endsWith('-')) return 'Don’t end the name with “-”'
  if (RESERVED_NAMES.has(name)) return `“${name}” is reserved`
  if (looksLikeId(name)) return 'This looks like a resource id; choose another name'
  if (taken.has(name)) return `A service or datastore named “${name}” already exists`
  return null
}

/** Postgres database / user name (`checks::pg_identifier`); empty = use the default. */
export function pgIdentifierError(value: string): string | null {
  if (value === '') return null
  if (value.length > 63) return 'Use at most 63 characters'
  if (/^[0-9]/.test(value)) return 'Don’t start with a digit'
  if (!/^[A-Za-z0-9_]+$/.test(value)) return 'Use letters, digits and “_” only'
  return null
}

/** Default Postgres database name for a datastore (`-` → `_`). */
export function defaultDatabaseName(name: string): string {
  return name.replace(/-/g, '_')
}

/** `${{datastore.NAME.PROP}}` */
export function datastoreRef(name: string, property = 'connectionString'): string {
  return `\${{datastore.${name}.${property}}}`
}

/** Properties a service env var can reference (`ferry_core::env`). */
export const DATASTORE_PROPERTIES: { property: string; description: string; postgresOnly?: boolean }[] = [
  { property: 'connectionString', description: 'Full URL on the private network' },
  { property: 'externalConnectionString', description: 'URL published on the Ferry host' },
  { property: 'host', description: 'Private hostname' },
  { property: 'port', description: 'Port on the private network' },
  { property: 'user', description: 'Username' },
  { property: 'password', description: 'Password' },
  { property: 'database', description: 'Database name', postgresOnly: true },
]

/**
 * Names of the datastores referenced in one env value — same syntax and kind
 * aliases as the server (`checks::references`).
 */
export function referencedDatastores(value: string): string[] {
  const out: string[] = []
  let rest = value
  for (;;) {
    const start = rest.indexOf('${{')
    if (start < 0) break
    const after = rest.slice(start + 3)
    const end = after.indexOf('}}')
    if (end < 0) break
    const parts = after
      .slice(0, end)
      .trim()
      .split('.')
      .map((p) => p.trim())
    if (parts.length === 3) {
      const [kind = '', name = ''] = parts
      if (['datastore', 'db', 'database'].includes(kind.toLowerCase())) out.push(name)
    }
    rest = after.slice(end + 2)
  }
  return out
}

/** Replace the password in a connection URL with bullets (display only). */
export function maskPassword(url: string, password: string): string {
  if (!password) return url
  return url.split(`:${password}@`).join(':••••••••@')
}
