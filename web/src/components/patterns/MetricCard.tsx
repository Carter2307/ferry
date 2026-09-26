import * as React from 'react'
import { Info } from 'lucide-react'

import { Skeleton } from '@/components/ui/skeleton'
import { Hint } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'

import { MonoLabel } from './MonoLabel'

interface MetricCardProps {
  label: React.ReactNode
  value?: React.ReactNode
  /** Small text after the value ("of 2", "/ 512 MiB"). */
  unit?: React.ReactNode
  /** (?) tooltip next to the label. */
  info?: React.ReactNode
  /** Right side of the header (legend, "12.5% errors ›"). */
  aside?: React.ReactNode
  loading?: boolean
  /** Chart / bar / extra content under the value. */
  children?: React.ReactNode
  /** Render as a compact tile (Observability grid). */
  compact?: boolean
  className?: string
}

/** Bordered metric card: mono label (+info) + big number (+ chart). */
export function MetricCard({ label, value, unit, info, aside, loading = false, children, compact = false, className }: MetricCardProps) {
  return (
    <div className={cn('flex min-w-0 flex-col gap-3 rounded-lg border bg-surface-100 shadow-card', compact ? 'p-4' : 'p-5', className)}>
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-center gap-1.5">
          <MonoLabel>{label}</MonoLabel>
          {info && (
            <Hint label={info}>
              <button type="button" className="rounded-full text-foreground-muted outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring" aria-label="More info">
                <Info className="size-3.5" />
              </button>
            </Hint>
          )}
        </div>
        {aside && <div className="flex items-center gap-3 text-[12px] text-foreground-lighter">{aside}</div>}
      </div>
      {loading ? (
        <Skeleton className="h-6 w-20" />
      ) : (
        value !== undefined && (
          <div className="flex items-baseline gap-1.5">
            <span className="text-xl text-foreground tabular md:text-[22px]">{value}</span>
            {unit && <span className="text-[13px] text-foreground-lighter">{unit}</span>}
          </div>
        )
      )}
      {children}
    </div>
  )
}

/** Thin horizontal usage bar (0–100). */
export function UsageBar({ value, tone = 'default', label }: { value: number | null | undefined; tone?: 'default' | 'warning' | 'destructive'; label?: string }) {
  const pct = value === null || value === undefined || !Number.isFinite(value) ? 0 : Math.max(0, Math.min(100, value))
  const auto = tone === 'default' ? (pct >= 90 ? 'destructive' : pct >= 75 ? 'warning' : 'default') : tone
  return (
    <div
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
      className="h-1.5 w-full overflow-hidden rounded-full bg-foreground/[0.07]"
    >
      <div
        className={cn(
          'h-full rounded-full transition-[width] duration-500',
          auto === 'destructive' ? 'bg-destructive-solid' : auto === 'warning' ? 'bg-chart-2' : 'bg-brand',
        )}
        style={{ width: `${pct}%` }}
      />
    </div>
  )
}

/** `● WARNINGS` legend item. */
export function LegendDot({ color, children }: { color: string; children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1.5 font-mono text-[11px] tracking-[0.06em] uppercase">
      <span aria-hidden="true" className="size-1.5 rounded-full" style={{ background: color }} />
      {children}
    </span>
  )
}
