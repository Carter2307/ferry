import { NavLink, type NavLinkProps } from 'react-router'
import { PanelLeftClose, PanelLeftOpen } from 'lucide-react'

import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { useUi } from '@/stores/ui'

import { NAV_ITEMS, type NavItem } from './nav'

function RailLink({ item, expanded, ...props }: { item: NavItem; expanded: boolean } & Omit<NavLinkProps, 'to'>) {
  return (
    <NavLink
      to={item.to}
      aria-label={expanded ? undefined : item.label}
      className={({ isActive }) =>
        cn(
          'group flex h-9 items-center gap-3 rounded-md px-[9px] text-sm outline-none transition-colors',
          'focus-visible:ring-2 focus-visible:ring-ring',
          isActive ? 'bg-selection text-foreground' : 'text-foreground-lighter hover:bg-surface-200 hover:text-foreground',
        )
      }
      {...props}
    >
      <item.icon className="size-[18px] shrink-0" strokeWidth={1.6} aria-hidden="true" />
      <span className={cn('truncate', expanded ? '' : 'sr-only')}>{item.label}</span>
    </NavLink>
  )
}

/**
 * Left icon rail (Studio): 56px, line icons in fg-lighter, active item on the
 * selection fill. Expands to show labels (toggle pinned at the bottom).
 */
export function IconRail({ className }: { className?: string }) {
  const expanded = useUi((s) => s.railExpanded)
  const toggle = useUi((s) => s.toggleRail)

  return (
    <nav
      aria-label="Main"
      className={cn(
        'flex h-full shrink-0 flex-col border-r bg-background py-2 transition-[width] duration-200 ease-out',
        expanded ? 'w-[200px]' : 'w-14',
        className,
      )}
    >
      <ul className="flex flex-1 flex-col gap-0.5 px-2">
        {NAV_ITEMS.map((group, gi) => (
          <li key={gi} className="flex flex-col gap-0.5">
            {gi > 0 && (
              <div aria-hidden="true" className={cn('my-1.5 h-px bg-border-strong', expanded ? 'mx-2' : 'mx-auto w-6')} />
            )}
            <ul className="flex flex-col gap-0.5">
              {group.map((item) => (
                <li key={item.to}>
                  {expanded ? (
                    <RailLink item={item} expanded />
                  ) : (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <RailLink item={item} expanded={false} />
                      </TooltipTrigger>
                      <TooltipContent side="right">{item.label}</TooltipContent>
                    </Tooltip>
                  )}
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ul>
      <div className="px-2">
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={toggle}
              aria-label={expanded ? 'Collapse menu' : 'Expand menu'}
              aria-expanded={expanded}
              className="flex h-9 w-full cursor-pointer items-center gap-3 rounded-md px-[9px] text-sm text-foreground-lighter transition-colors outline-none hover:bg-surface-200 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
            >
              {expanded ? (
                <PanelLeftClose className="size-[18px] shrink-0" strokeWidth={1.6} aria-hidden="true" />
              ) : (
                <PanelLeftOpen className="size-[18px] shrink-0" strokeWidth={1.6} aria-hidden="true" />
              )}
              <span className={cn(expanded ? '' : 'sr-only')}>Collapse</span>
            </button>
          </TooltipTrigger>
          <TooltipContent side="right">{expanded ? 'Collapse menu' : 'Expand menu'}</TooltipContent>
        </Tooltip>
      </div>
    </nav>
  )
}
