import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { keys } from '../keys'
import { invalidateService } from './services'

/** `GET /api/v1/services/{ref}/domains` — custom domains. */
export function useServiceDomains(ref: string | undefined) {
  return useQuery({
    queryKey: keys.serviceDomains(ref ?? ''),
    queryFn: ({ signal }) => endpoints.listDomains(ref ?? '', signal),
    enabled: Boolean(ref),
  })
}

/** `POST /api/v1/services/{ref}/domains` — `mutate(domain)`. */
export function useAddDomain(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (domain: string) => endpoints.addDomain(ref, domain),
    onSuccess: (domains) => {
      qc.setQueryData(keys.serviceDomains(ref), domains)
      void invalidateService(qc, ref)
    },
  })
}

/** `DELETE /api/v1/services/{ref}/domains/{domain}` — `mutate(domain)`. */
export function useRemoveDomain(ref: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (domain: string) => endpoints.removeDomain(ref, domain),
    onMutate: async (domain) => {
      // optimistic: hide the row immediately, roll back on error
      await qc.cancelQueries({ queryKey: keys.serviceDomains(ref) })
      const previous = qc.getQueryData<string[]>(keys.serviceDomains(ref))
      qc.setQueryData<string[]>(keys.serviceDomains(ref), (d) => d?.filter((x) => x !== domain))
      return { previous }
    },
    onError: (_err, _domain, ctx) => {
      if (ctx?.previous) qc.setQueryData(keys.serviceDomains(ref), ctx.previous)
    },
    onSuccess: (domains) => {
      qc.setQueryData(keys.serviceDomains(ref), domains)
      void invalidateService(qc, ref)
    },
  })
}
