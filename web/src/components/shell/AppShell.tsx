import * as React from 'react'
import { Outlet, useLocation } from 'react-router'

import { useChangeFeed } from '@/lib/api/events'
import { useUi } from '@/stores/ui'

import { CommandMenu } from './CommandMenu'
import { IconRail } from './IconRail'
import { MobileNav } from './MobileNav'
import { TopBar } from './TopBar'
import { useCommandShortcut } from './useCommandShortcut'

/**
 * Authenticated layout: top bar, icon rail (desktop) / sheet (phones), the
 * routed page, the ⌘K palette and the live change-feed subscription.
 */
export function AppShell() {
  useChangeFeed()
  useCommandShortcut()
  const mobileOpen = useUi((s) => s.mobileNavOpen)
  const setMobileOpen = useUi((s) => s.setMobileNavOpen)
  const { pathname } = useLocation()
  const mainRef = React.useRef<HTMLElement | null>(null)

  // New page: scroll the content back to the top.
  React.useEffect(() => {
    mainRef.current?.scrollTo({ top: 0 })
  }, [pathname])

  return (
    <div className="relative flex h-dvh flex-col overflow-hidden bg-background">
      <a
        href="#main"
        className="sr-only z-50 rounded-md bg-surface-300 px-3 py-2 text-sm focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:ring-2 focus:ring-ring"
      >
        Skip to content
      </a>
      <TopBar onOpenMobileNav={() => setMobileOpen(true)} />
      <div className="flex min-h-0 flex-1">
        <IconRail className="hidden md:flex" />
        <main id="main" ref={mainRef} tabIndex={-1} className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto outline-none">
          <Outlet />
        </main>
      </div>
      <MobileNav open={mobileOpen} onOpenChange={setMobileOpen} />
      <CommandMenu />
    </div>
  )
}
