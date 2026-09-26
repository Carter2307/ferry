import * as React from 'react'
import { NavLink } from 'react-router'
import { ArrowUpRight } from 'lucide-react'

import { MonoLabel } from '@/components/patterns/MonoLabel'
import { cn } from '@/lib/utils'

export interface InnerMenuItem {
  label: string
  /** Route (relative or absolute) or external URL when `external`. */
  to: string
  icon?: React.ReactNode
  /** Pill after the label (e.g. <Badge variant="warning">BETA</Badge>). */
  badge?: React.ReactNode
  /** Opens in a new tab with a ↗. */
  external?: boolean
  /** Match the route exactly (index routes). */
  end?: boolean
}

export interface InnerMenuGroup {
  /** UPPERCASE mono group label (optional for the first group). */
  label?: string
  items: InnerMenuItem[]
}

interface InnerMenuProps {
  title: React.ReactNode
  groups: InnerMenuGroup[]
  /** Content under the groups (e.g. a dashed empty-state card, danger link). */
  footer?: React.ReactNode
  /** Accessible name of the navigation. */
  label?: string
  className?: string
}

/**
 * Studio inner side menu (Settings / Observability): title header, groups
 * separated by borders with mono labels, 30px items, active = selection fill.
 * Below `md` it turns into a horizontally scrolling tab strip.
 */
export function InnerMenu({ title, groups, footer, label, className }: InnerMenuProps) {
  const all = groups.flatMap((g) => g.items)
  return (
    <>
      {/* desktop: side menu */}
      <aside
        aria-label={label ?? (typeof title === 'string' ? title : 'Section')}
        className={cn('hidden w-[240px] shrink-0 flex-col border-r bg-background md:flex xl:w-[260px]', className)}
      >
        <div className="flex h-12 shrink-0 items-center border-b px-6">
          <h2 className="truncate text-[15px] font-medium text-foreground">{title}</h2>
        </div>
        <nav className="relative flex min-h-0 flex-1 flex-col overflow-y-auto" aria-label={label ?? undefined}>
          {groups.map((group, i) => (
            <div key={group.label ?? i} className="flex flex-col gap-0.5 border-b px-3 py-4 last:border-b-0">
              {group.label && <MonoLabel as="h3" className="mb-1.5 px-3">{group.label}</MonoLabel>}
              <ul className="flex flex-col gap-0.5">
                {group.items.map((item) => (
                  <li key={item.to}>
                    <InnerMenuLink item={item} />
                  </li>
                ))}
              </ul>
            </div>
          ))}
          {footer && <div className="mt-auto px-3 py-4">{footer}</div>}
        </nav>
      </aside>

      {/* mobile: tab strip */}
      <nav
        aria-label={label ?? (typeof title === 'string' ? title : 'Section')}
        className="sticky top-0 z-10 flex shrink-0 gap-1 overflow-x-auto border-b bg-background px-3 py-2 scrollbar-none md:hidden"
      >
        {all.map((item) =>
          item.external ? (
            <a
              key={item.to}
              href={item.to}
              target="_blank"
              rel="noreferrer"
              className="inline-flex h-8 shrink-0 items-center gap-1 rounded-full px-3 text-[13px] whitespace-nowrap text-foreground-light outline-none hover:bg-surface-200 focus-visible:ring-2 focus-visible:ring-ring"
            >
              {item.label}
              <ArrowUpRight className="size-3" aria-hidden="true" />
            </a>
          ) : (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.end}
              className={({ isActive }) =>
                cn(
                  'inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full border px-3 text-[13px] whitespace-nowrap outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                  isActive
                    ? 'border-border-strong bg-selection font-medium text-foreground'
                    : 'border-transparent text-foreground-light hover:bg-surface-200 hover:text-foreground',
                )
              }
            >
              {item.label}
              {item.badge}
            </NavLink>
          ),
        )}
      </nav>
    </>
  )
}

function InnerMenuLink({ item }: { item: InnerMenuItem }) {
  const base =
    'group flex h-[30px] items-center gap-2 rounded-md px-3 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring'
  if (item.external) {
    return (
      <a
        href={item.to}
        target="_blank"
        rel="noreferrer"
        className={cn(base, 'text-foreground-light hover:bg-surface-200 hover:text-foreground')}
      >
        {item.icon && <span className="text-foreground-lighter [&_svg]:size-4">{item.icon}</span>}
        <span className="truncate">{item.label}</span>
        {item.badge}
        <ArrowUpRight className="ml-auto size-3.5 text-foreground-lighter" aria-hidden="true" />
        <span className="sr-only">(opens in a new tab)</span>
      </a>
    )
  }
  return (
    <NavLink
      to={item.to}
      end={item.end}
      className={({ isActive }) =>
        cn(
          base,
          isActive
            ? 'bg-selection font-medium text-foreground'
            : 'text-foreground-light hover:bg-surface-200 hover:text-foreground',
        )
      }
    >
      {item.icon && <span className="text-foreground-lighter group-aria-[current=page]:text-foreground [&_svg]:size-4">{item.icon}</span>}
      <span className="truncate">{item.label}</span>
      {item.badge && <span className="ml-auto">{item.badge}</span>}
    </NavLink>
  )
}
