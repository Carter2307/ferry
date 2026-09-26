import { NavLink } from 'react-router'
import { LogOut, Monitor, Moon, Sun } from 'lucide-react'

import { FerryLogo } from '@/components/patterns/icons'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useServerInfo } from '@/lib/api/queries'
import { cn } from '@/lib/utils'
import { useAuth } from '@/stores/auth'
import { useUi, type ThemePreference } from '@/stores/ui'

import { ConnectPopover } from './ConnectPopover'
import { NAV_ITEMS } from './nav'

/** Phone navigation: the icon rail becomes a left sheet with labels, theme and sign out. */
export function MobileNav({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const theme = useUi((s) => s.theme)
  const setTheme = useUi((s) => s.setTheme)
  const signOut = useAuth((s) => s.signOut)
  const { data: info } = useServerInfo()

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent side="left" className="w-[280px] max-w-[85vw] p-0">
        <SheetHeader>
          <SheetTitle className="flex items-center gap-2">
            <FerryLogo className="size-5" /> Ferry
          </SheetTitle>
          <SheetDescription>{info ? `${info.base_domain} · v${info.version}` : 'Self-hosted deploys'}</SheetDescription>
        </SheetHeader>
        <nav aria-label="Main" className="flex flex-1 flex-col gap-4 overflow-y-auto px-3 py-4">
          {NAV_ITEMS.map((group, gi) => (
            <ul key={gi} className={cn('flex flex-col gap-0.5', gi > 0 && 'border-t pt-4')}>
              {group.map((item) => (
                <li key={item.to}>
                  <NavLink
                    to={item.to}
                    onClick={() => onOpenChange(false)}
                    className={({ isActive }) =>
                      cn(
                        'flex h-10 items-center gap-3 rounded-md px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring',
                        isActive ? 'bg-selection font-medium text-foreground' : 'text-foreground-light hover:bg-surface-200',
                      )
                    }
                  >
                    <item.icon className="size-[18px]" strokeWidth={1.6} aria-hidden="true" />
                    {item.label}
                  </NavLink>
                </li>
              ))}
            </ul>
          ))}
          <div className="flex flex-col gap-2 border-t pt-4">
            <MonoLabel className="px-1">Connect</MonoLabel>
            <div>
              <ConnectPopover />
            </div>
          </div>
          <div className="flex flex-col gap-2 border-t pt-4">
            <MonoLabel className="px-1">Theme</MonoLabel>
            <ToggleGroup
              type="single"
              variant="outline"
              value={theme}
              onValueChange={(v) => v && setTheme(v as ThemePreference)}
              aria-label="Theme"
            >
              <ToggleGroupItem value="light" aria-label="Light">
                <Sun /> Light
              </ToggleGroupItem>
              <ToggleGroupItem value="dark" aria-label="Dark">
                <Moon /> Dark
              </ToggleGroupItem>
              <ToggleGroupItem value="system" aria-label="System">
                <Monitor /> Auto
              </ToggleGroupItem>
            </ToggleGroup>
          </div>
        </nav>
        <div className="border-t p-3">
          <Button className="w-full" icon={<LogOut />} onClick={signOut}>
            Sign out
          </Button>
        </div>
      </SheetContent>
    </Sheet>
  )
}
