import { useMutation, useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'

import { ApiError } from '../client'
import { endpoints } from '../endpoints'
import { usePollInterval } from '../events'
import { keys } from '../keys'
import type { CreateService, Deploy, ServiceView, UpdateService } from '../types'

/** Find a service in the cached list by name or id (instant placeholder for details). */
export function cachedService(qc: QueryClient, ref: string): ServiceView | undefined {
  return qc.getQueryData<ServiceView[]>(keys.serviceList())?.find((s) => s.name === ref || s.id === ref)
}

/** Store a fresh ServiceView everywhere it is cached (detail + list). */
export function putService(qc: QueryClient, ref: string, view: ServiceView): void {
  qc.setQueryData(keys.service(ref), view)
  if (ref !== view.name) qc.setQueryData(keys.service(view.name), view)
  qc.setQueryData<ServiceView[]>(keys.serviceList(), (list) =>
    list ? list.map((s) => (s.id === view.id ? view : s)) : list,
  )
}

/** Refresh everything about one service (after deploys, restarts, env changes…). */
export function invalidateService(qc: QueryClient, ref: string): Promise<void> {
  return Promise.all([
    qc.invalidateQueries({ queryKey: keys.service(ref) }),
    qc.invalidateQueries({ queryKey: keys.serviceList() }),
  ]).then(() => undefined)
}

/** `GET /api/v1/services` (polls every 5s without the change feed). */
export function useServices() {
  const refetchInterval = usePollInterval(5000)
  return useQuery({
    queryKey: keys.serviceList(),
    queryFn: ({ signal }) => endpoints.listServices(signal),
    refetchInterval,
  })
}

/** `GET /api/v1/services/{ref}` — placeholder from the list cache while loading. */
export function useService(ref: string | undefined) {
  const qc = useQueryClient()
  const refetchInterval = usePollInterval(3000)
  return useQuery<ServiceView, Error, ServiceView, ReturnType<typeof keys.service>>({
    queryKey: keys.service(ref ?? ''),
    queryFn: ({ signal }) => endpoints.getService(ref ?? '', signal),
    enabled: Boolean(ref),
    placeholderData: () => (ref ? cachedService(qc, ref) : undefined),
    // stop polling a service that no longer exists
    refetchInterval: (q) => (q.state.error instanceof ApiError && q.state.error.isNotFound ? false : refetchInterval),
  })
}

/** `GET /api/v1/services/{ref}/status` — live instances with CPU / memory (always polled). */
export function useServiceStatus(ref: string | undefined, opts: { enabled?: boolean; intervalMs?: number } = {}) {
  const refetchInterval = usePollInterval(opts.intervalMs ?? 5000, { always: true })
  return useQuery({
    queryKey: keys.serviceStatus(ref ?? ''),
    queryFn: ({ signal }) => endpoints.serviceStatus(ref ?? '', signal),
    enabled: Boolean(ref) && (opts.enabled ?? true),
    refetchInterval,
  })
}

/** `POST /api/v1/services` */
export function useCreateService() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateService) => endpoints.createService(body),
    onSuccess: (view) => {
      qc.setQueryData(keys.service(view.name), view)
      void qc.invalidateQueries({ queryKey: keys.serviceList() })
      if (view.env_groups.length > 0) void qc.invalidateQueries({ queryKey: keys.envGroups() })
    },
  })
}

/** `PATCH /api/v1/services/{ref}` */
export function useUpdateService(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: UpdateService) => endpoints.updateService(ref, body),
    onSuccess: (view) => {
      putService(qc, ref, view)
      void invalidateService(qc, ref)
    },
  })
}

/** `DELETE /api/v1/services/{ref}?force=` (409 while referenced unless force). */
export function useDeleteService() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ ref, force = false }: { ref: string; force?: boolean }) => endpoints.deleteService(ref, force),
    onSuccess: (_void, { ref }) => {
      const cached = cachedService(qc, ref)
      qc.setQueryData<ServiceView[]>(keys.serviceList(), (list) =>
        list?.filter((s) => s.name !== ref && s.id !== ref),
      )
      qc.removeQueries({ queryKey: keys.service(ref) })
      if (cached) qc.removeQueries({ queryKey: keys.service(cached.name) })
      void qc.invalidateQueries({ queryKey: keys.services() })
      void qc.invalidateQueries({ queryKey: keys.envGroups() })
    },
  })
}

function useDeployingAction(ref: string, fn: () => Promise<Deploy>) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: fn,
    onSuccess: (deploy) => {
      qc.setQueryData(keys.deploy(deploy.id), deploy)
      void invalidateService(qc, ref)
    },
  })
}

/** `POST /api/v1/services/{ref}/restart` → 202 Deploy */
export function useRestartService(ref: string) {
  return useDeployingAction(ref, () => endpoints.restartService(ref))
}

/** `POST /api/v1/services/{ref}/rollback` → 202 Deploy (only to deploys that went live). */
export function useRollbackService(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (deployId: string) => endpoints.rollbackService(ref, deployId),
    onSuccess: (deploy) => {
      qc.setQueryData(keys.deploy(deploy.id), deploy)
      void invalidateService(qc, ref)
    },
  })
}

function useServiceViewAction<V>(ref: string, fn: (vars: V) => Promise<ServiceView>) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: fn,
    onSuccess: (view) => {
      putService(qc, ref, view)
      void invalidateService(qc, ref)
    },
  })
}

/** `POST /api/v1/services/{ref}/suspend` */
export function useSuspendService(ref: string) {
  return useServiceViewAction<void>(ref, () => endpoints.suspendService(ref))
}

/** `POST /api/v1/services/{ref}/resume` */
export function useResumeService(ref: string) {
  return useServiceViewAction<void>(ref, () => endpoints.resumeService(ref))
}

/** `POST /api/v1/services/{ref}/scale` — `mutate(instances)` */
export function useScaleService(ref: string) {
  return useServiceViewAction<number>(ref, (instances) => endpoints.scaleService(ref, instances))
}

/** `POST /api/v1/services/{ref}/deploy-hook/rotate` — old hook URL stops working. */
export function useRotateDeployHook(ref: string) {
  return useServiceViewAction<void>(ref, () => endpoints.rotateDeployHook(ref))
}
