import * as React from 'react'

import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'

import { MonoLabel } from './MonoLabel'

interface InfoTileProps {
  icon: React.ReactNode
  label: React.ReactNode
  value: React.ReactNode
  /** Secondary line under the value. */
  hint?: React.ReactNode
  loading?: boolean
  className?: string
}

/** Project-home tile: square outlined icon box + mono label + value. */
export function InfoTile({ icon, label, value, hint, loading = false, className }: InfoTileProps) {
  return (
    <div className={cn('flex min-w-0 items-center gap-4', className)}>
      <div
        aria-hidden="true"
        className="flex size-14 shrink-0 items-center justify-center rounded-lg border bg-surface-100 text-foreground-light shadow-card md:size-[72px] [&_svg]:size-5"
      >
        {icon}
      </div>
      <div className="flex min-w-0 flex-col gap-1">
        <MonoLabel>{label}</MonoLabel>
        {loading ? (
          <Skeleton className="h-5 w-28" />
        ) : (
          <div className="min-w-0 truncate text-[15px] text-foreground md:text-[17px]">{value}</div>
        )}
        {hint && !loading && <div className="truncate text-[13px] text-foreground-lighter">{hint}</div>}
      </div>
    </div>
  )
}
