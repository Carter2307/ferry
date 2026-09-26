import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { usePollInterval } from '../events'
import { keys } from '../keys'
import { isJobTerminal, type RunJobRequest } from '../types'

/** `GET /api/v1/services/{ref}/jobs?limit=` */
export function useJobs(ref: string | undefined, limit = 20) {
  const refetchInterval = usePollInterval(3000)
  return useQuery({
    queryKey: keys.serviceJobs(ref ?? '', limit),
    queryFn: ({ signal }) => endpoints.listJobs(ref ?? '', limit, signal),
    enabled: Boolean(ref),
    refetchInterval,
  })
}

/** `GET /api/v1/jobs/{id}` — polls while running (without the change feed). */
export function useJob(id: string | undefined) {
  const poll = usePollInterval(2000)
  return useQuery({
    queryKey: keys.job(id ?? ''),
    queryFn: ({ signal }) => endpoints.getJob(id ?? '', signal),
    enabled: Boolean(id),
    refetchInterval: (q) => (q.state.data && !isJobTerminal(q.state.data.status) ? poll : false),
  })
}

/** `POST /api/v1/services/{ref}/jobs` — `mutate({command?})` (required for non-cron services). */
export function useRunJob(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: RunJobRequest = {}) => endpoints.runJob(ref, body),
    onSuccess: (job) => {
      qc.setQueryData(keys.job(job.id), job)
      void qc.invalidateQueries({ queryKey: keys.serviceJobs(ref) })
    },
  })
}

/** `POST /api/v1/jobs/{id}/cancel` — `mutate(jobId)`. Older servers answer 404. */
export function useCancelJob() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (jobId: string) => endpoints.cancelJob(jobId),
    onSuccess: (job) => {
      qc.setQueryData(keys.job(job.id), job)
      void qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === 'services' && q.queryKey[3] === 'jobs' })
    },
  })
}
