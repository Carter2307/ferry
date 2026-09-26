/**
 * Pure helpers for the Environment page: the merged ("effective") environment
 * of a service and `${{…}}` reference parsing. They mirror
 * `ferry-core/src/env.rs` (`merge`, `resolve_value`, `injected`).
 */

import { isPublicHttp, listensOnPort, sourceKind, type EnvVar, type ServiceView } from '@/lib/api/types'

export type LayerKind = 'ferry' | 'group' | 'service'

/** One source of variables, lowest priority first. */
export interface EnvLayer {
  /** Stable id (`ferry`, `group:<name>`, `service`). */
  id: string
  kind: LayerKind
  /** Display name (`Ferry`, the group name, `This service`). */
  label: string
  vars: EnvVar[]
}

export interface EffectiveVar {
  key: string
  value: string
  /** The layer whose value wins. */
  source: EnvLayer
  /** Lower layers that also define the key (their value is replaced). */
  overridden: EnvLayer[]
}

/**
 * Merge layers like `env::merge`: later layers override earlier ones and
 * keys keep the order of their first appearance.
 */
export function mergeLayers(layers: EnvLayer[]): EffectiveVar[] {
  const out: EffectiveVar[] = []
  const index = new Map<string, number>()
  for (const layer of layers) {
    for (const v of layer.vars) {
      const at = index.get(v.key)
      const existing = at === undefined ? undefined : out[at]
      if (existing) {
        out[at as number] = {
          key: v.key,
          value: v.value,
          source: layer,
          overridden: existing.source.id === layer.id ? existing.overridden : [...existing.overridden, existing.source],
        }
      } else {
        index.set(v.key, out.length)
        out.push({ key: v.key, value: v.value, source: layer, overridden: [] })
      }
    }
  }
  return out
}

/** Placeholder value for injected variables only known once a deploy runs. */
export const PER_DEPLOY = '(set at each deploy)'

/**
 * Variables Ferry injects into every container (`env::injected`); the
 * user's variables override them. Values that depend on the deploy that
 * starts the container are shown as `PER_DEPLOY`.
 */
export function injectedVars(service: ServiceView): EnvVar[] {
  const vars: EnvVar[] = [
    { key: 'FERRY', value: 'true' },
    { key: 'FERRY_SERVICE_ID', value: service.id },
    { key: 'FERRY_SERVICE_NAME', value: service.name },
    { key: 'FERRY_SERVICE_TYPE', value: service.type },
    { key: 'FERRY_DEPLOY_ID', value: PER_DEPLOY },
    { key: 'FERRY_INTERNAL_HOSTNAME', value: service.name },
  ]
  if (listensOnPort(service.type)) {
    vars.push({ key: 'PORT', value: service.internal_port !== null ? String(service.internal_port) : PER_DEPLOY })
  }
  if (sourceKind(service) === 'git') {
    vars.push({ key: 'FERRY_GIT_COMMIT', value: PER_DEPLOY })
    vars.push({ key: 'FERRY_GIT_BRANCH', value: service.branch })
  }
  if (isPublicHttp(service.type) && service.url) {
    const defaultHost = service.hosts.find((h) => !service.custom_domains.includes(h)) ?? service.hosts[0]
    if (defaultHost) vars.push({ key: 'FERRY_EXTERNAL_HOSTNAME', value: defaultHost })
    vars.push({ key: 'FERRY_EXTERNAL_URL', value: service.url })
  }
  return vars
}

// ---------------------------------------------------------------------------
// References: ${{kind.name.property}}
// ---------------------------------------------------------------------------

export const DATASTORE_PROPS = [
  'connectionString',
  'externalConnectionString',
  'host',
  'port',
  'user',
  'password',
  'database',
] as const

/** Every property name `resolve_reference` accepts for a datastore (incl. aliases). */
const DATASTORE_PROP_ALIASES = new Set<string>([
  ...DATASTORE_PROPS,
  'url',
  'internalUrl',
  'externalUrl',
  'hostname',
  'username',
  'databaseName',
])

export const SERVICE_PROPS = ['hostport', 'host', 'port', 'internalUrl', 'url'] as const

const SERVICE_PROP_ALIASES = new Set<string>([...SERVICE_PROPS, 'hostname', 'externalUrl'])

export interface EnvReference {
  /** The text as written, e.g. `${{ datastore.db.connectionString }}`. */
  raw: string
  kind: 'datastore' | 'service' | null
  name: string
  property: string
  /** Why it would fail at container start (null when it looks valid). */
  problem: string | null
}

export interface KnownTargets {
  datastores: readonly string[]
  services: readonly string[]
}

/**
 * Find `${{…}}` references in a value (skipping `$${{` escapes) and check
 * them against the known datastores / services. `known` = null skips the
 * name check (lists not loaded).
 */
export function findReferences(value: string, known: KnownTargets | null): EnvReference[] {
  const refs: EnvReference[] = []
  let rest = value
  while (true) {
    const start = rest.indexOf('${{')
    if (start < 0) break
    if (start > 0 && rest[start - 1] === '$') {
      rest = rest.slice(start + 3)
      continue
    }
    const after = rest.slice(start + 3)
    const end = after.indexOf('}}')
    if (end < 0) {
      refs.push({ raw: rest.slice(start), kind: null, name: '', property: '', problem: 'Unterminated reference (missing }})' })
      break
    }
    const expr = after.slice(0, end).trim()
    const raw = rest.slice(start, start + 3 + end + 2)
    refs.push(checkReference(raw, expr, known))
    rest = after.slice(end + 2)
  }
  return refs
}

function checkReference(raw: string, expr: string, known: KnownTargets | null): EnvReference {
  const parts = expr.split('.').map((p) => p.trim())
  if (parts.length !== 3) {
    return { raw, kind: null, name: '', property: '', problem: 'Expected ${{kind.name.property}}' }
  }
  const [kindRaw = '', name = '', property = ''] = parts
  const kindLower = kindRaw.toLowerCase()
  if (kindLower === 'datastore' || kindLower === 'db' || kindLower === 'database') {
    let problem: string | null = null
    if (known && !known.datastores.includes(name)) problem = `Unknown datastore “${name}”`
    else if (!DATASTORE_PROP_ALIASES.has(property)) problem = `Unknown datastore property “${property}”`
    return { raw, kind: 'datastore', name, property, problem }
  }
  if (kindLower === 'service' || kindLower === 'svc') {
    let problem: string | null = null
    if (known && !known.services.includes(name)) problem = `Unknown service “${name}”`
    else if (!SERVICE_PROP_ALIASES.has(property)) problem = `Unknown service property “${property}”`
    return { raw, kind: 'service', name, property, problem }
  }
  return { raw, kind: null, name, property, problem: `Unknown reference kind “${kindRaw}” (use datastore or service)` }
}

/** `${{datastore.NAME.PROP}}` */
export function datastoreRef(name: string, prop: string = 'connectionString'): string {
  return `\${{datastore.${name}.${prop}}}`
}

/** `${{service.NAME.PROP}}` */
export function serviceRef(name: string, prop: string = 'hostport'): string {
  return `\${{service.${name}.${prop}}}`
}

/** Conventional variable name for a datastore reference (`DATABASE_URL`, `REDIS_URL`, `CACHE_URL`…). */
export function suggestedKey(kind: 'postgres' | 'redis', name: string, taken: readonly string[]): string {
  const base = kind === 'postgres' ? 'DATABASE_URL' : 'REDIS_URL'
  if (!taken.includes(base)) return base
  const fromName = `${name.toUpperCase().replace(/[^A-Z0-9]+/g, '_').replace(/^_+|_+$/g, '')}_URL`
  if (!taken.includes(fromName)) return fromName
  let i = 2
  while (taken.includes(`${fromName}_${i}`)) i++
  return `${fromName}_${i}`
}
