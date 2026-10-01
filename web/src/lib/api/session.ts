/**
 * The dashboard's session with the server: asking whether there is one, and
 * ending it. (Signing in is the sign-in and setup pages' own call; the cookie
 * itself is HttpOnly and never passes through here.)
 */

import { useAuth } from '@/stores/auth'

import { errorMessage } from './client'
import { endpoints } from './endpoints'

/** Ask the server where this browser stands; the answer lands in the auth store. */
export async function loadAuthStatus(): Promise<void> {
  try {
    useAuth.getState().apply(await endpoints.authStatus())
  } catch (e) {
    useAuth.getState().unreachable(errorMessage(e))
  }
}

/** End the session on the server, then here whatever the server said. */
export async function signOut(): Promise<void> {
  try {
    await endpoints.logout()
  } catch {
    /* unreachable or already ended: this browser forgets the session either way */
  }
  useAuth.getState().signedOut()
}
