import { useMutation, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '@/lib/api/endpoints'
import { keys } from '@/lib/api/keys'
import { invalidateService, putService } from '@/lib/api/queries'

const ID_PREFIXES = ['srv', 'dep', 'job', 'dbs', 'evg']

/** `validate::env_group_name`: 1–64 of `[A-Za-z0-9_.-]`, not id-shaped. Null when valid. */
export function envGroupNameError(name: string, taken: ReadonlySet<string> = new Set()): string | null {
  if (name === '') return 'Name is required'
  if (name.length > 64) return 'Use at most 64 characters'
  if (!/^[A-Za-z0-9_.-]+$/.test(name)) return 'Use letters, digits, “-”, “_” or “.” only'
  if (ID_PREFIXES.some((p) => name.startsWith(`${p}-`) && /^[0-9a-f]*$/.test(name.slice(p.length + 1)))) {
    return 'This looks like a resource id; choose another name'
  }
  if (taken.has(name)) return `An env group named “${name}” already exists`
  return null
}

/** Link / unlink a service to the env group `group` (`POST|DELETE /services/{s}/env-groups/{g}`). */
export function useServiceGroupLink(group: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ service, link }: { service: string; link: boolean }) =>
      link ? endpoints.linkEnvGroup(service, group) : endpoints.unlinkEnvGroup(service, group),
    onSuccess: (view, { service }) => {
      putService(qc, service, view)
      void invalidateService(qc, service)
      void qc.invalidateQueries({ queryKey: keys.envGroups() })
    },
  })
}
