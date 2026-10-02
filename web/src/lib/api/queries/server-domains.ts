import { useMutation, useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'

import { certificateOnItsWay } from '@/lib/domains'

import { endpoints } from '../endpoints'
import { usePollInterval } from '../events'
import { keys } from '../keys'
import type { DomainView } from '../types'

/**
 * A domain that starts or stops being served changes the default domain of
 * `/info` and the hosts and URL of every service.
 */
function invalidateDomains(qc: QueryClient): void {
  void qc.invalidateQueries({ queryKey: keys.domains() })
  void qc.invalidateQueries({ queryKey: keys.info() })
  void qc.invalidateQueries({ queryKey: keys.services() })
}

function upsert(qc: QueryClient, domain: DomainView): void {
  qc.setQueryData<DomainView[]>(keys.domainList(), (list) => {
    if (!list) return list
    return list.some((d) => d.id === domain.id) ? list.map((d) => (d.id === domain.id ? domain : d)) : [...list, domain]
  })
}

/**
 * `GET /api/v1/domains` — the domains services are served under: the
 * server's base domain and the ones connected to it.
 */
export function useServerDomains() {
  const refetchInterval = usePollInterval(5_000)
  return useQuery({
    queryKey: keys.domainList(),
    queryFn: ({ signal }) => endpoints.listServerDomains(signal),
    refetchInterval,
  })
}

/** `POST /api/v1/domains` — `mutate(name)`. */
export function useConnectDomain() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (name: string) => endpoints.connectDomain(name),
    onSuccess: (domain) => {
      upsert(qc, domain)
      invalidateDomains(qc)
    },
  })
}

/** `POST /api/v1/domains/{id}/verify` — check now whether its names reach the server. */
export function useVerifyDomain() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => endpoints.verifyDomain(id),
    onSuccess: (domain) => {
      upsert(qc, domain)
      invalidateDomains(qc)
    },
  })
}

/** `PATCH /api/v1/domains/{id}` `{is_default: true}` — the domain service URLs are shown with. */
export function useSetDefaultDomain() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => endpoints.setDefaultDomain(id),
    onSuccess: (domain) => {
      qc.setQueryData<DomainView[]>(keys.domainList(), (list) =>
        list?.map((d) => (d.id === domain.id ? domain : { ...d, is_default: false })),
      )
      invalidateDomains(qc)
    },
  })
}

/** `DELETE /api/v1/domains/{id}` (409 for the base domain). */
export function useDisconnectDomain() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => endpoints.disconnectDomain(id),
    onSuccess: (_void, id) => {
      qc.setQueryData<DomainView[]>(keys.domainList(), (list) => list?.filter((d) => d.id !== id))
      invalidateDomains(qc)
    },
  })
}

/**
 * `GET /api/v1/certificates` — the certificate of every routed hostname.
 * Certificates are not in the change feed: asked often while one is on its
 * way, now and then otherwise.
 */
export function useCertificates({ enabled = true }: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: keys.certificates(),
    queryFn: ({ signal }) => endpoints.listCertificates(signal),
    enabled,
    refetchInterval: (query) => (query.state.data?.some(certificateOnItsWay) ? 3_000 : 30_000),
  })
}
