import * as React from 'react'

/**
 * Collects dirty flags from several forms of one page:
 * `const dirty = useDirtyRegistry()` → `dirty.set('build', true)` → `dirty.any`.
 */
export function useDirtyRegistry() {
  const [flags, setFlags] = React.useState<Record<string, boolean>>({})
  const set = React.useCallback((id: string, value: boolean) => {
    setFlags((prev) => (Boolean(prev[id]) === value ? prev : { ...prev, [id]: value }))
  }, [])
  const any = Object.values(flags).some(Boolean)
  return { any, set }
}

/** Report a form's dirty flag to a `useDirtyRegistry()` owner. */
export function useReportDirty(report: ((id: string, dirty: boolean) => void) | undefined, id: string, dirty: boolean) {
  React.useEffect(() => {
    report?.(id, dirty)
  }, [report, id, dirty])
  React.useEffect(() => () => report?.(id, false), [report, id])
}

/** Current time, re-rendering every `ms` (running durations, relative times). */
export function useNow(ms = 1000): number {
  const [now, setNow] = React.useState(() => Date.now())
  React.useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), ms)
    return () => window.clearInterval(t)
  }, [ms])
  return now
}

/**
 * Form state bound to a server value (render-time sync, no effects):
 * - while the form is pristine, server changes flow into it;
 * - while it is dirty, the user's edits are kept and `stale` tells that the
 *   server value changed underneath.
 * `commit(v)` = the server now holds `v` (after a successful save).
 */
export function useSyncedForm<F extends object>(server: F, equals: (a: F, b: F) => boolean) {
  const [base, setBase] = React.useState<F>(server)
  const [value, setValue] = React.useState<F>(server)
  const [seen, setSeen] = React.useState<F>(server)
  const dirty = !equals(value, base)
  if (!equals(server, seen)) {
    setSeen(server)
    if (!dirty) {
      setBase(server)
      setValue(server)
    }
  }
  const patch = React.useCallback((p: Partial<F>) => setValue((v) => ({ ...v, ...p })), [])
  const reset = React.useCallback(() => setValue(base), [base])
  const commit = React.useCallback((v: F) => {
    setBase(v)
    setValue(v)
    setSeen(v)
  }, [])
  const loadLatest = React.useCallback(() => {
    setBase(seen)
    setValue(seen)
  }, [seen])
  return { value, base, dirty, stale: dirty && !equals(seen, base), patch, reset, commit, loadLatest }
}
