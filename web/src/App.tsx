import * as React from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider } from 'react-router'

import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import { ApiError } from '@/lib/api/client'
import { loadAuthStatus } from '@/lib/api/session'
import { ServerUnreachable } from '@/pages/login/ServerUnreachable'
import { useAuth } from '@/stores/auth'
import { useResolvedTheme } from '@/stores/ui'

import { FullPageLoader } from './FullPageLoader'
import { createRouter } from './routes'

const ReactQueryDevtools = import.meta.env.DEV
  ? React.lazy(() => import('@tanstack/react-query-devtools').then((m) => ({ default: m.ReactQueryDevtools })))
  : null

function makeQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 10_000,
        gcTime: 5 * 60_000,
        refetchOnWindowFocus: true,
        // Never retry client errors (404, 401, 409…); retry network / 5xx twice.
        retry: (count, error) => {
          if (error instanceof ApiError && error.status >= 400 && error.status < 500) return false
          return count < 2
        },
      },
      mutations: { retry: false },
    },
  })
}

/** Applies `.dark` on <html> from the theme preference (+ system changes). */
function ThemeSync() {
  const theme = useResolvedTheme()
  React.useEffect(() => {
    const root = document.documentElement
    // Switch instantly: suspend CSS transitions for one frame so colors don't animate.
    const style = document.createElement('style')
    style.textContent = '*,*::before,*::after{transition:none!important}'
    document.head.appendChild(style)
    root.classList.toggle('dark', theme === 'dark')
    root.style.colorScheme = theme
    void window.getComputedStyle(document.body).opacity // force a style flush
    const raf = requestAnimationFrame(() => style.remove())
    return () => {
      cancelAnimationFrame(raf)
      style.remove()
    }
  }, [theme])
  return null
}

/** Drop every cached response when the user signs out (or another account signs in). */
function useClearCacheOnSignOut(qc: QueryClient) {
  React.useEffect(
    () =>
      useAuth.subscribe((state, prev) => {
        if (state.user?.id !== prev.user?.id) qc.clear()
      }),
    [qc],
  )
}

/** Ask the server once whether it has an account and whether this browser is signed in. */
function useAuthStatus() {
  React.useEffect(() => {
    void loadAuthStatus()
  }, [])
}

/** The pages, once the server said where this browser stands. */
function Pages({ router }: { router: ReturnType<typeof createRouter> }) {
  const phase = useAuth((s) => s.phase)
  if (phase === 'loading') return <FullPageLoader />
  if (phase === 'unreachable') return <ServerUnreachable />
  return <RouterProvider router={router} />
}

export function App() {
  const [queryClient] = React.useState(makeQueryClient)
  const [router] = React.useState(createRouter)
  useClearCacheOnSignOut(queryClient)
  useAuthStatus()

  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <ThemeSync />
        <Pages router={router} />
        <Toaster />
      </TooltipProvider>
      {ReactQueryDevtools && (
        <React.Suspense fallback={null}>
          <ReactQueryDevtools buttonPosition="bottom-right" />
        </React.Suspense>
      )}
    </QueryClientProvider>
  )
}
