import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { usePollInterval } from '../events'
import { keys } from '../keys'
import type { CreateEnvGroup, EnvGroupView, EnvVar } from '../types'

/** `GET /api/v1/env-groups` */
export function useEnvGroups() {
  const refetchInterval = usePollInterval(10_000)
  return useQuery({
    queryKey: keys.envGroupList(),
    queryFn: ({ signal }) => endpoints.listEnvGroups(signal),
    refetchInterval,
  })
}

/** `GET /api/v1/env-groups/{ref}` */
export function useEnvGroup(ref: string | undefined) {
  const qc = useQueryClient()
  return useQuery<EnvGroupView, Error, EnvGroupView, ReturnType<typeof keys.envGroup>>({
    queryKey: keys.envGroup(ref ?? ''),
    queryFn: ({ signal }) => endpoints.getEnvGroup(ref ?? '', signal),
    enabled: Boolean(ref),
    placeholderData: () =>
      qc.getQueryData<EnvGroupView[]>(keys.envGroupList())?.find((g) => g.name === ref || g.id === ref),
  })
}

function putGroup(qc: ReturnType<typeof useQueryClient>, ref: string, group: EnvGroupView): void {
  qc.setQueryData(keys.envGroup(ref), group)
  if (ref !== group.name) qc.setQueryData(keys.envGroup(group.name), group)
  qc.setQueryData<EnvGroupView[]>(keys.envGroupList(), (list) =>
    list ? list.map((g) => (g.id === group.id ? group : g)) : list,
  )
}

/** `POST /api/v1/env-groups` */
export function useCreateEnvGroup() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateEnvGroup) => endpoints.createEnvGroup(body),
    onSuccess: (group) => {
      qc.setQueryData(keys.envGroup(group.name), group)
      void qc.invalidateQueries({ queryKey: keys.envGroupList() })
    },
  })
}

/** `DELETE /api/v1/env-groups/{ref}?force=&restart=` (409 while linked unless force). */
export function useDeleteEnvGroup() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ ref, force = false, restart = false }: { ref: string; force?: boolean; restart?: boolean }) =>
      endpoints.deleteEnvGroup(ref, { force, restart }),
    onSuccess: (_v, { ref }) => {
      qc.setQueryData<EnvGroupView[]>(keys.envGroupList(), (list) =>
        list?.filter((g) => g.name !== ref && g.id !== ref),
      )
      qc.removeQueries({ queryKey: keys.envGroup(ref) })
      void qc.invalidateQueries({ queryKey: keys.envGroups() })
      void qc.invalidateQueries({ queryKey: keys.services() })
    },
  })
}

/** `PUT /api/v1/env-groups/{ref}/env?restart=` — replace all vars. */
export function useReplaceEnvGroupEnv(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ vars, restart = false }: { vars: EnvVar[]; restart?: boolean }) =>
      endpoints.replaceEnvGroupEnv(ref, { vars }, restart),
    onSuccess: (group, { restart }) => {
      putGroup(qc, ref, group)
      if (restart) void qc.invalidateQueries({ queryKey: keys.services() })
    },
  })
}

/** `PATCH /api/v1/env-groups/{ref}/env?restart=` — upsert `set`, delete `unset`. */
export function usePatchEnvGroupEnv(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ set = [], unset = [], restart = false }: { set?: EnvVar[]; unset?: string[]; restart?: boolean }) =>
      endpoints.patchEnvGroupEnv(ref, { set, unset }, restart),
    onSuccess: (group, { restart }) => {
      putGroup(qc, ref, group)
      if (restart) void qc.invalidateQueries({ queryKey: keys.services() })
    },
  })
}
