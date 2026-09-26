import { create } from 'zustand'

import { safeStorage } from '@/lib/storage'

/** Same key as the legacy single-file dashboard, so existing sessions carry over. */
export const TOKEN_KEY = 'ferry.token'

interface AuthState {
  /** API token sent as `Authorization: Bearer …`; null when signed out. */
  token: string | null
  /** Store a validated token. */
  signIn: (token: string) => void
  /** Forget the token (the router guard then redirects to /login). */
  signOut: () => void
}

export const useAuth = create<AuthState>()((set) => ({
  token: safeStorage.get(TOKEN_KEY) || null,
  signIn: (token) => {
    const t = token.trim()
    safeStorage.set(TOKEN_KEY, t)
    set({ token: t })
  },
  signOut: () => {
    safeStorage.remove(TOKEN_KEY)
    set({ token: null })
  },
}))

// Keep tabs in sync: signing out in one tab signs out the others.
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (e) => {
    if (e.key === TOKEN_KEY) useAuth.setState({ token: e.newValue || null })
  })
}

/** Current token outside React (API client). */
export function getToken(): string | null {
  return useAuth.getState().token
}
