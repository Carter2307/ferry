import * as React from 'react'
import { Link } from 'react-router'
import { ArrowUpRight } from 'lucide-react'

import { MonoLabel } from '@/components/patterns/MonoLabel'
import { cn } from '@/lib/utils'

export interface ServerMenuItem {
  id: string
  label: string
  icon: React.ReactNode
}

export interface ServerMenuLink {
  label: string
  href: string
}

/**
 * Inner side menu of the Server page. Same look as `shell/InnerMenu`, but its
 * items select a `?section=` (the route has no sub-paths), so the active item
 * is driven by `current` instead of the pathname.
 */
export function ServerMenu({
  items,
  links,
  current,
}: {
  items: ServerMenuItem[]
  links: ServerMenuLink[]
  current: string
}) {
  const href = (id: string) => (id === items[0]?.id ? '/server' : `/server?section=${id}`)
  return (
    <>
      <aside
        aria-label="Server settings"
        className="hidden w-[240px] shrink-0 flex-col border-r bg-background md:flex xl:w-[260px]"
      >
        <div className="flex h-12 shrink-0 items-center border-b px-6">
          <h2 className="truncate text-[15px] font-medium text-foreground">Server</h2>
        </div>
        <nav aria-label="Server sections" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
          <div className="flex flex-col gap-0.5 border-b px-3 py-4">
            <MonoLabel as="h3" className="mb-1.5 px-3">
              Settings
            </MonoLabel>
            <ul className="flex flex-col gap-0.5">
              {items.map((item) => {
                const active = item.id === current
                return (
                  <li key={item.id}>
                    <Link
                      to={href(item.id)}
                      aria-current={active ? 'page' : undefined}
                      className={cn(
                        'group flex h-[30px] items-center gap-2 rounded-md px-3 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                        active
                          ? 'bg-selection font-medium text-foreground'
                          : 'text-foreground-light hover:bg-surface-200 hover:text-foreground',
                      )}
                    >
                      <span
                        className={cn('[&_svg]:size-4', active ? 'text-foreground' : 'text-foreground-lighter')}
                        aria-hidden="true"
                      >
                        {item.icon}
                      </span>
                      <span className="truncate">{item.label}</span>
                    </Link>
                  </li>
                )
              })}
            </ul>
          </div>
          <div className="flex flex-col gap-0.5 px-3 py-4">
            <MonoLabel as="h3" className="mb-1.5 px-3">
              Resources
            </MonoLabel>
            <ul className="flex flex-col gap-0.5">
              {links.map((l) => (
                <li key={l.href}>
                  <a
                    href={l.href}
                    target="_blank"
                    rel="noreferrer"
                    className="flex h-[30px] items-center gap-2 rounded-md px-3 text-sm text-foreground-light outline-none transition-colors hover:bg-surface-200 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    <span className="truncate">{l.label}</span>
                    <ArrowUpRight className="ml-auto size-3.5 text-foreground-lighter" aria-hidden="true" />
                    <span className="sr-only">(opens in a new tab)</span>
                  </a>
                </li>
              ))}
            </ul>
          </div>
        </nav>
      </aside>

      {/* phones: horizontally scrolling tab strip */}
      <nav
        aria-label="Server sections"
        className="sticky top-0 z-10 flex shrink-0 gap-1 overflow-x-auto border-b bg-background px-3 py-2 scrollbar-none md:hidden"
      >
        {items.map((item) => {
          const active = item.id === current
          return (
            <Link
              key={item.id}
              to={href(item.id)}
              aria-current={active ? 'page' : undefined}
              className={cn(
                'inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full border px-3 text-[13px] whitespace-nowrap outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring',
                active
                  ? 'border-border-strong bg-selection font-medium text-foreground'
                  : 'border-transparent text-foreground-light hover:bg-surface-200 hover:text-foreground',
              )}
            >
              {item.label}
            </Link>
          )
        })}
      </nav>
    </>
  )
}
