import { useQueries } from '@tanstack/react-query'

import { endpoints } from '@/lib/api/endpoints'
import { keys } from '@/lib/api/keys'
import { useEnvGroups, useServices } from '@/lib/api/queries'
import type { ServiceView } from '@/lib/api/types'

import { referencedDatastores } from './lib'

export interface DatastoreReference {
  service: ServiceView
  /** Env var keys whose value references the datastore. */
  keys: string[]
  /** Set when the reference comes from a linked env group. */
  group?: string
}

/**
 * Services whose environment references the datastore `name` through
 * `${{datastore.NAME.…}}` — their own variables or a linked env group's.
 * (There is no server endpoint for this; it scans the envs client-side,
 * the same way the server decides a delete is referenced.)
 */
export function useDatastoreReferences(name: string | undefined) {
  const services = useServices()
  const groups = useEnvGroups()
  const list = services.data ?? []
  const envs = useQueries({
    queries: list.map((s) => ({
      queryKey: keys.serviceEnv(s.name),
      queryFn: ({ signal }: { signal: AbortSignal }) => endpoints.getServiceEnv(s.name, signal),
      enabled: Boolean(name),
      staleTime: 15_000,
    })),
  })

  // Cheap (a few services × a few vars): recomputed on every render.
  const refs: DatastoreReference[] = []
  if (name) {
    list.forEach((service, i) => {
      const vars = envs[i]?.data ?? []
      const own = vars.filter((v) => referencedDatastores(v.value).includes(name)).map((v) => v.key)
      if (own.length > 0) refs.push({ service, keys: own })
    })
    for (const g of groups.data ?? []) {
      const groupKeys = g.vars.filter((v) => referencedDatastores(v.value).includes(name)).map((v) => v.key)
      if (groupKeys.length === 0) continue
      for (const svcName of g.services) {
        const service = list.find((s) => s.name === svcName)
        if (service) refs.push({ service, keys: groupKeys, group: g.name })
      }
    }
    refs.sort((a, b) => a.service.name.localeCompare(b.service.name))
  }

  const loading = services.isLoading || groups.isLoading || envs.some((q) => q.isLoading)
  const error = services.error ?? groups.error ?? envs.find((q) => q.error)?.error ?? null
  return { refs, loading, error }
}
