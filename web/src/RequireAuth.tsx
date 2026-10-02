import { Navigate } from 'react-router'

import { AppShell } from '@/components/shell/AppShell'
import { useAuthRedirect } from '@/lib/useAuthRedirect'

/** Redirects to /setup or /login?next=… when there is no session. */
export function RequireAuth() {
  const redirect = useAuthRedirect()
  if (redirect) return <Navigate to={redirect} replace />
  return <AppShell />
}
