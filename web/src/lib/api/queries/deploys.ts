import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { usePollInterval } from '../events'
import { keys } from '../keys'
import { isDeployActive, type Deploy, type TriggerDeploy } from '../types'
import { invalidateService } from './services'

/** `GET /api/v1/services/{ref}/deploys?limit=` — newest first. */
export function useDeploys(ref: string | undefined, limit = 20) {
  const refetchInterval = usePollInterval(3000)
  return useQuery({
    queryKey: keys.serviceDeploys(ref ?? '', limit),
    queryFn: ({ signal }) => endpoints.listDeploys(ref ?? '', limit, signal),
    enabled: Boolean(ref),
    refetchInterval,
  })
}

/** `GET /api/v1/deploys/{id}` — polls while the deploy is active (without the change feed). */
export function useDeploy(id: string | undefined) {
  const poll = usePollInterval(2000)
  return useQuery({
    queryKey: keys.deploy(id ?? ''),
    queryFn: ({ signal }) => endpoints.getDeploy(id ?? '', signal),
    enabled: Boolean(id),
    refetchInterval: (q) => (q.state.data && isDeployActive(q.state.data.status) ? poll : false),
  })
}

/** `POST /api/v1/services/{ref}/deploys` — manual deploy (`{commit?, clear_cache?}`). */
export function useTriggerDeploy(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: TriggerDeploy = {}) => endpoints.triggerDeploy(ref, body),
    onSuccess: (deploy) => {
      qc.setQueryData(keys.deploy(deploy.id), deploy)
      void invalidateService(qc, ref)
    },
  })
}

/** `POST /api/v1/deploys/{id}/cancel` — `mutate(deployId)`. */
export function useCancelDeploy() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (deployId: string) => endpoints.cancelDeploy(deployId),
    onSuccess: (deploy: Deploy) => {
      qc.setQueryData(keys.deploy(deploy.id), deploy)
      void qc.invalidateQueries({ queryKey: keys.services() })
    },
  })
}
