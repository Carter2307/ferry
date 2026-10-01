import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { usePollInterval } from '../events'
import { keys } from '../keys'
import type { AuthorizeGit, ConnectGit, GitCallback, GitConnectionView } from '../types'

/** What the provider says is asked again at most this often (and on Refresh). */
const PROVIDER_STALE_MS = 60_000

/**
 * `GET /api/v1/git/connections` — the GitHub / GitLab accounts this server
 * is authorized to read (including the ones whose authorization is pending).
 */
export function useGitConnections() {
  const refetchInterval = usePollInterval(10_000)
  return useQuery({
    queryKey: keys.gitConnections(),
    queryFn: ({ signal }) => endpoints.listGitConnections(signal),
    refetchInterval,
  })
}

/**
 * `POST /api/v1/git/authorize` — where to send the browser to authorize an
 * account (or the connection, when nothing is left to do).
 */
export function useAuthorizeGit() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: AuthorizeGit) => endpoints.authorizeGit(body),
    // A pending connection may have appeared, or one just got ready.
    onSuccess: () => void qc.invalidateQueries({ queryKey: keys.git() }),
  })
}

/** `POST /api/v1/git/callback` — hand over what the provider sent the browser back with. */
export function useGitCallback() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: GitCallback) => endpoints.gitCallback(body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: keys.git() }),
  })
}

/**
 * `POST /api/v1/git/connections` — connect the account a personal access
 * token belongs to (the alternative to authorizing in the browser).
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
      // What can be read (repositories, branches) changes with the accounts.
      void qc.invalidateQueries({ queryKey: keys.git() })
    },
  })
}

/** `DELETE /api/v1/git/connections/{id}?force=` (409 while it clones for services unless force). */
export function useDisconnectGit() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, force = false }: { id: string; force?: boolean }) => endpoints.deleteGitConnection(id, force),
    onSuccess: (_void, { id }) => {
      qc.setQueryData<GitConnectionView[]>(keys.gitConnections(), (list) => list?.filter((c) => c.id !== id))
      qc.removeQueries({ queryKey: keys.gitRepositories(id) })
      void qc.invalidateQueries({ queryKey: keys.git() })
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

/**
 * `GET /api/v1/git/branches?repo_url=` — the branches of any repository,
 * asked from its remote (with the connection that serves it, if any).
 */
export function useGitBranches(repoUrl: string | undefined, opts: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: keys.gitBranches(repoUrl ?? ''),
    queryFn: ({ signal }) => endpoints.listGitBranches(repoUrl ?? '', signal),
    enabled: Boolean(repoUrl) && (opts.enabled ?? true),
    staleTime: PROVIDER_STALE_MS,
    refetchOnWindowFocus: false,
    // A repository that can't be read is an answer too: no retry storm while typing.
    retry: false,
  })
}
