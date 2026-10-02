import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { endpoints } from '../endpoints'
import { keys } from '../keys'
import type { ApiTokenView, ChangePassword, CliLoginView, CreateApiToken, SessionView } from '../types'

/** `GET /api/v1/auth/sessions` — the browsers signed in to the account. */
export function useSessions() {
  return useQuery({ queryKey: keys.authSessions(), queryFn: ({ signal }) => endpoints.listSessions(signal) })
}

/** `DELETE /api/v1/auth/sessions/{id}` — sign another browser out. */
export function useEndSession() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => endpoints.endSession(id),
    onSuccess: (_void, id) => {
      qc.setQueryData<SessionView[]>(keys.authSessions(), (list) => list?.filter((s) => s.id !== id))
    },
  })
}

/** `POST /api/v1/auth/password` — every other session ends. */
export function useChangePassword() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: ChangePassword) => endpoints.changePassword(body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: keys.authSessions() }),
  })
}

/** `GET /api/v1/auth/tokens` — the named API tokens (never the tokens themselves). */
export function useApiTokens() {
  return useQuery({ queryKey: keys.authTokens(), queryFn: ({ signal }) => endpoints.listApiTokens(signal) })
}

/** `POST /api/v1/auth/tokens` — the answer is the only time the token is shown. */
export function useCreateApiToken() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateApiToken) => endpoints.createApiToken(body),
    onSuccess: (created) => {
      qc.setQueryData<ApiTokenView[]>(keys.authTokens(), (list) => list && [created.api_token, ...list])
    },
  })
}

/** `DELETE /api/v1/auth/tokens/{id}` */
export function useRevokeApiToken() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => endpoints.revokeApiToken(id),
    onSuccess: (_void, id) => {
      qc.setQueryData<ApiTokenView[]>(keys.authTokens(), (list) => list?.filter((t) => t.id !== id))
    },
  })
}

/** `GET /api/v1/auth/cli/{id}` — a `ferry login` waiting for its approval. */
export function useCliLogin(id: string | undefined) {
  return useQuery({
    queryKey: keys.authCliLogin(id ?? ''),
    queryFn: ({ signal }) => endpoints.getCliLogin(id ?? '', signal),
    enabled: Boolean(id),
    // An answered or expired request is gone for good.
    retry: false,
    refetchOnWindowFocus: false,
  })
}

/** `POST /api/v1/auth/cli/{id}/approve` or `/deny`. */
export function useAnswerCliLogin(id: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (approve: boolean) => (approve ? endpoints.approveCliLogin(id) : endpoints.denyCliLogin(id)),
    onSuccess: (view) => {
      qc.setQueryData<CliLoginView>(keys.authCliLogin(id), view)
      // Approving made an API token.
      void qc.invalidateQueries({ queryKey: keys.authTokens() })
    },
  })
}
