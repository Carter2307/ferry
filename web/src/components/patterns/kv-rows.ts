import * as React from 'react'

import type { EnvVar } from '@/lib/api/types'
import { isValidEnvKey } from '@/lib/dotenv'

/** Editor row (stable `id` for React keys / focus). */
export interface KvRow {
  id: string
  key: string
  value: string
}

/** Fixed-length mask for hidden values (one dot per character would leak the length). */
export const VALUE_MASK = '••••••••••••'

/**
 * The value is only `${{…}}` references (e.g. `${{datastore.app-db.connectionString}}`):
 * not a secret itself, so editors show it in clear.
 */
export function isReferenceOnly(value: string): boolean {
  return /^(?:\$\{\{[^{}]+\}\})+$/.test(value.trim())
}

let rowSeq = 0
/** Fresh row id. */
export function newRowId(): string {
  rowSeq += 1
  return `kv-${rowSeq}`
}

export function rowsFromEnv(vars: EnvVar[]): KvRow[] {
  return vars.map((v) => ({ id: newRowId(), key: v.key, value: v.value }))
}

/** Rows → EnvVar[] (blank rows dropped, keys trimmed). */
export function envFromRows(rows: KvRow[]): EnvVar[] {
  return rows.filter((r) => r.key.trim() !== '' || r.value !== '').map((r) => ({ key: r.key.trim(), value: r.value }))
}

/** Per-row validation messages (invalid / duplicate / missing key). */
export function validateRows(rows: KvRow[]): Map<string, string> {
  const errors = new Map<string, string>()
  const seen = new Map<string, string>()
  for (const r of rows) {
    const key = r.key.trim()
    if (key === '' && r.value === '') continue
    if (key === '') errors.set(r.id, 'Key is required')
    else if (!isValidEnvKey(key)) errors.set(r.id, 'Use letters, digits and _ (not starting with a digit)')
    else if (seen.has(key)) errors.set(r.id, `Duplicate key ${key}`)
    else seen.set(key, r.id)
  }
  return errors
}

/** Upsert `incoming` into `rows` by key (existing rows keep their position). */
export function mergeRows(rows: KvRow[], incoming: EnvVar[]): KvRow[] {
  const next = rows.filter((r) => r.key.trim() !== '' || r.value !== '').map((r) => ({ ...r }))
  for (const v of incoming) {
    const existing = next.find((r) => r.key.trim() === v.key)
    if (existing) existing.value = v.value
    else next.push({ id: newRowId(), key: v.key, value: v.value })
  }
  return next
}

function sameVars(a: EnvVar[], b: EnvVar[]): boolean {
  return a.length === b.length && a.every((v, i) => v.key === b[i]?.key && v.value === b[i]?.value)
}

/**
 * State for a KeyValueEditor bound to server data: `reset(vars)` when the
 * server value changes, `vars` for the request body, `dirty` / `valid` for the
 * Save button.
 */
export function useKeyValueRows(initial: EnvVar[] | undefined) {
  const [base, setBase] = React.useState<EnvVar[]>(initial ?? [])
  const [rows, setRows] = React.useState<KvRow[]>(() => rowsFromEnv(initial ?? []))
  const reset = React.useCallback((vars: EnvVar[]) => {
    setBase(vars)
    setRows(rowsFromEnv(vars))
  }, [])
  const vars = React.useMemo(() => envFromRows(rows), [rows])
  const errors = React.useMemo(() => validateRows(rows), [rows])
  return {
    rows,
    setRows,
    reset,
    vars,
    errors,
    valid: errors.size === 0,
    dirty: !sameVars(vars, base),
  }
}
