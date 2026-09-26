import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { keys } from '../keys'
import type { EnvVar } from '../types'
import { invalidateService, putService } from './services'

/** `GET /api/v1/services/{ref}/env` */
export function useServiceEnv(ref: string | undefined) {
  return useQuery({
    queryKey: keys.serviceEnv(ref ?? ''),
    queryFn: ({ signal }) => endpoints.getServiceEnv(ref ?? '', signal),
    enabled: Boolean(ref),
  })
}

/** `PUT /api/v1/services/{ref}/env?restart=` — replace all vars. */
export function useReplaceServiceEnv(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ vars, restart = false }: { vars: EnvVar[]; restart?: boolean }) =>
      endpoints.replaceServiceEnv(ref, { vars }, restart),
    onSuccess: (vars) => {
      qc.setQueryData(keys.serviceEnv(ref), vars)
      void invalidateService(qc, ref)
    },
  })
}

/** `PATCH /api/v1/services/{ref}/env?restart=` — upsert `set`, delete `unset`. */
export function usePatchServiceEnv(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ set = [], unset = [], restart = false }: { set?: EnvVar[]; unset?: string[]; restart?: boolean }) =>
      endpoints.patchServiceEnv(ref, { set, unset }, restart),
    onSuccess: (vars) => {
      qc.setQueryData(keys.serviceEnv(ref), vars)
      void invalidateService(qc, ref)
    },
  })
}

/** `POST /api/v1/services/{ref}/env-groups` — `mutate(groupNameOrId)`. */
export function useLinkEnvGroup(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (group: string) => endpoints.linkEnvGroup(ref, group),
    onSuccess: (view) => {
      putService(qc, ref, view)
      void invalidateService(qc, ref)
      void qc.invalidateQueries({ queryKey: keys.envGroups() })
    },
  })
}

/** `DELETE /api/v1/services/{ref}/env-groups/{group}` — `mutate(groupNameOrId)`. */
export function useUnlinkEnvGroup(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (group: string) => endpoints.unlinkEnvGroup(ref, group),
    onSuccess: (view) => {
      putService(qc, ref, view)
      void invalidateService(qc, ref)
      void qc.invalidateQueries({ queryKey: keys.envGroups() })
    },
  })
}
