import * as React from 'react'
import { Link } from 'react-router'

import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'

/** Card grid shared by the resource lists (services, datastores, env groups). */
export const RESOURCE_GRID = 'grid grid-cols-[repeat(auto-fill,minmax(min(100%,248px),1fr))] gap-4'

interface ResourceCardProps {
  /** Where the card leads (the whole card is clickable through a stretched link). */
  href: string
  name: string
  /** 16px line icon next to the name (resource type / kind). */
  icon: React.ReactNode
  /** Kebab menu (rendered above the stretched link). */
  menu?: React.ReactNode
  /** Line under the name (source, kind, keys…), fg-lighter 13px. */
  subtitle?: React.ReactNode
  /** Row of mono badges. */
  badges?: React.ReactNode
  /** Bottom of the card: endpoint line, status row (`(✓) Service is live`)… */
  footer?: React.ReactNode
  className?: string
}

/**
 * Studio project card, one anatomy for every resource list: 20px padding,
 * name 15px/500 with a 16px icon and a ⋮ menu, subtitle, mono badges, and a
 * footer pinned to the bottom with the status row.
 */
export function ResourceCard({ href, name, icon, menu, subtitle, badges, footer, className }: ResourceCardProps) {
  return (
    <li
      className={cn(
        'group/card relative flex min-h-[188px] flex-col rounded-lg border bg-surface-100 p-5 shadow-card transition-colors',
        'hover:border-border-stronger hover:bg-surface-75 dark:hover:bg-surface-300',
        'has-[a[data-card-link]:focus-visible]:ring-2 has-[a[data-card-link]:focus-visible]:ring-ring',
        className,
      )}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2.5">
          <span aria-hidden="true" className="flex shrink-0 text-foreground-lighter [&_svg]:size-4">
            {icon}
          </span>
          <h3 className="min-w-0 truncate text-[15px] leading-6 font-medium text-foreground">
            {/* Stretched link: the whole card opens the resource. */}
            <Link to={href} data-card-link className="outline-none after:absolute after:inset-0 after:rounded-lg after:content-['']">
              {name}
            </Link>
          </h3>
        </div>
        {menu && <div className="relative z-10 -mt-0.5 -mr-1.5 shrink-0">{menu}</div>}
      </div>

      {subtitle && (
        <div className="mt-1 flex min-w-0 items-center gap-1.5 text-[13px] text-foreground-lighter">{subtitle}</div>
      )}

      {badges && <div className="mt-3 flex flex-wrap items-center gap-1.5">{badges}</div>}

      {footer && <div className="mt-auto flex min-w-0 flex-col gap-2.5 pt-5">{footer}</div>}
    </li>
  )
}

/** Placeholder card while a list loads. */
export function ResourceCardSkeleton() {
  return (
    <li className="flex min-h-[188px] flex-col rounded-lg border bg-surface-100 p-5 shadow-card" aria-hidden="true">
      <div className="flex items-center gap-2.5">
        <Skeleton className="size-4 rounded-sm" />
        <Skeleton className="h-4 w-28" />
      </div>
      <Skeleton className="mt-2.5 h-3.5 w-40" />
      <div className="mt-3.5 flex gap-1.5">
        <Skeleton className="h-5 w-10 rounded-sm" />
        <Skeleton className="h-5 w-12 rounded-sm" />
      </div>
      <div className="mt-auto flex flex-col gap-3 pt-5">
        <Skeleton className="h-3.5 w-36" />
        <div className="flex items-center gap-2">
          <Skeleton className="size-5 rounded-full" />
          <Skeleton className="h-3.5 w-24" />
        </div>
      </div>
    </li>
  )
}

/**
 * Card footer status: small circular outlined icon + label, like the Studio
 * project card (`(⏸) Project is paused`). `ring` sets the circle's border and
 * icon colour.
 */
export function CardStatusLine({
  icon,
  ring,
  spin = false,
  children,
  className,
}: {
  icon: React.ReactNode
  ring: string
  spin?: boolean
  children: React.ReactNode
  className?: string
}) {
  return (
    <span className={cn('inline-flex min-w-0 items-center gap-2 text-[13px] text-foreground-light', className)}>
      <span
        aria-hidden="true"
        className={cn(
          'flex size-5 shrink-0 items-center justify-center rounded-full border bg-surface-100 [&_svg]:size-3 [&_svg]:stroke-[2.5]',
          spin && '[&_svg]:animate-spin',
          ring,
        )}
      >
        {icon}
      </span>
      <span className="truncate">{children}</span>
    </span>
  )
}
