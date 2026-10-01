import { useLocation } from 'react-router'

import { useAuth } from '@/stores/auth'

/** Where to go instead of a page that needs the session; `null` when signed in. */
export function useAuthRedirect(): string | null {
  const phase = useAuth((s) => s.phase)
  const location = useLocation()
  if (phase === 'signed-in') return null
  // A server without an account: create it first.
  if (phase === 'setup') return '/setup'
  const next = `${location.pathname}${location.search}${location.hash}`
  return `/login?next=${encodeURIComponent(next)}`
}
