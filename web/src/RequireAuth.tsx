import { Navigate, useLocation } from 'react-router'

import { AppShell } from '@/components/shell/AppShell'
import { useAuth } from '@/stores/auth'

/** Redirects to /login?next=… when signed out. */
export function RequireAuth() {
  const token = useAuth((s) => s.token)
  const location = useLocation()
  if (!token) {
    const next = `${location.pathname}${location.search}${location.hash}`
    return <Navigate to={`/login?next=${encodeURIComponent(next)}`} replace />
  }
  return <AppShell />
}
