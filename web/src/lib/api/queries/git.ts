import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { usePollInterval } from '../events'
import { keys } from '../keys'
import type { ConnectGit, GitConnectionView } from '../types'

/** What the provider says is asked again at most this often (and on Refresh). */
const PROVIDER_STALE_MS = 60_000

/** `GET /api/v1/git/connections` — the connected GitHub / GitLab accounts. */
export function useGitConnections() {
  const refetchInterval = usePollInterval(10_000)
  return useQuery({
    queryKey: keys.gitConnections(),
    queryFn: ({ signal }) => endpoints.listGitConnections(signal),
    refetchInterval,
  })
}

/**
 * `POST /api/v1/git/connections` — connect the account a token belongs to (or
 * replace the token of an account that is already connected).
 */
export function useConnectGit() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: ConnectGit) => endpoints.connectGit(body),
    onSuccess: (connection) => {
      qc.setQueryData<GitConnectionView[]>(keys.gitConnections(), (list) => {
        if (!list) return list
        return list.some((c) => c.id === connection.id)
          ? list.map((c) => (c.id === connection.id ? connection : c))
          : [...list, connection]
      })
      void qc.invalidateQueries({ queryKey: keys.gitConnections() })
      // A new token may see other repositories (or work again).
      void qc.resetQueries({ queryKey: keys.gitProvider(connection.id) })
    },
  })
}

/** `DELETE /api/v1/git/connections/{id}?force=` (409 while services use it unless force). */
export function useDisconnectGit() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, force = false }: { id: string; force?: boolean }) => endpoints.deleteGitConnection(id, force),
    onSuccess: (_void, { id }) => {
      qc.setQueryData<GitConnectionView[]>(keys.gitConnections(), (list) => list?.filter((c) => c.id !== id))
      qc.removeQueries({ queryKey: keys.gitProvider(id) })
      void qc.invalidateQueries({ queryKey: keys.gitConnections() })
      // Services that used it lost their `git_connection_id`.
      void qc.invalidateQueries({ queryKey: keys.services() })
    },
  })
}

/** `GET /api/v1/git/connections/{id}/repositories` — asked from the provider. */
export function useGitRepositories(id: string | undefined, opts: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: keys.gitRepositories(id ?? ''),
    queryFn: ({ signal }) => endpoints.listGitRepositories(id ?? '', signal),
    enabled: Boolean(id) && (opts.enabled ?? true),
    staleTime: PROVIDER_STALE_MS,
    refetchOnWindowFocus: false,
  })
}

/** `GET /api/v1/git/connections/{id}/branches?repository=` — asked from the provider. */
export function useGitBranches(id: string | undefined, repository: string | undefined, opts: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: keys.gitBranches(id ?? '', repository ?? ''),
    queryFn: ({ signal }) => endpoints.listGitBranches(id ?? '', repository ?? '', signal),
    enabled: Boolean(id) && Boolean(repository) && (opts.enabled ?? true),
    staleTime: PROVIDER_STALE_MS,
    refetchOnWindowFocus: false,
  })
}
