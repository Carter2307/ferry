import { create } from 'zustand'

import type { AuthStatus, UserView } from '@/lib/api/types'
import { safeStorage } from '@/lib/storage'

/**
 * Where the dashboard stands with the server's account:
 * `loading` until the server said, `unreachable` when it couldn't be asked,
 * `setup` while the server has no account, then `signed-out` / `signed-in`.
 */
export type AuthPhase = 'loading' | 'unreachable' | 'setup' | 'signed-out' | 'signed-in'

interface AuthState {
  phase: AuthPhase
  /** The account, while signed in. */
  user: UserView | null
  /** Why the server couldn't be asked (`unreachable`). */
  error: string | null
  /** Take what `GET /api/v1/auth/status` (or signing in) answered. */
  apply: (status: AuthStatus) => void
  /** The session is gone: signed out here, ended elsewhere, or expired. */
  signedOut: () => void
  unreachable: (message: string) => void
}

/**
 * The session itself is an HttpOnly cookie the browser keeps: no script, this
 * one included, can read it. The store only knows whether there is one.
 */
export const useAuth = create<AuthState>()((set) => ({
  phase: 'loading',
  user: null,
  error: null,
  apply: (status) => {
    if (status.setup_required) set({ phase: 'setup', user: null, error: null })
    else if (status.auth === 'session' && status.user) set({ phase: 'signed-in', user: status.user, error: null })
    else set({ phase: 'signed-out', user: null, error: null })
  },
  signedOut: () => set((s) => (s.phase === 'signed-in' ? { phase: 'signed-out', user: null } : s)),
  unreachable: (message) => set({ phase: 'unreachable', user: null, error: message }),
}))

/** True while signed in: what the pages, the change feed and the log streams watch. */
export function useSignedIn(): boolean {
  return useAuth((s) => s.phase === 'signed-in')
}

// Earlier versions kept the server's API token here: forget it.
safeStorage.remove('ferry.token')
