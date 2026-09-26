import * as React from 'react'
import { AlertTriangle, RefreshCw } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { errorMessage } from '@/lib/api/client'
import { cn } from '@/lib/utils'

interface EmptyStateProps {
  icon?: React.ReactNode
  title: React.ReactNode
  description?: React.ReactNode
  /** CTA buttons. */
  actions?: React.ReactNode
  /** `dashed` (default) = dashed border box; `bordered` = solid card; `plain` = no box. */
  variant?: 'dashed' | 'bordered' | 'plain'
  size?: 'sm' | 'md' | 'lg'
  className?: string
  /** Extra content (e.g. a CLI snippet) under the actions. */
  children?: React.ReactNode
}

/** Centered icon + title + text + CTA, in a dashed box (Studio empty state). */
export function EmptyState({
  icon,
  title,
  description,
  actions,
  variant = 'dashed',
  size = 'md',
  className,
  children,
}: EmptyStateProps) {
  return (
    <div
      className={cn(
        'flex w-full flex-col items-center justify-center gap-3 rounded-lg text-center',
        variant === 'dashed' && 'border border-dashed border-border-stronger',
        variant === 'bordered' && 'border bg-surface-100 shadow-card',
        size === 'sm' && 'px-4 py-6',
        size === 'md' && 'px-6 py-10',
        size === 'lg' && 'px-6 py-16',
        className,
      )}
    >
      {icon && (
        <div aria-hidden="true" className="flex items-center justify-center text-foreground-lighter [&_svg]:size-[18px]">
          {icon}
        </div>
      )}
      <div className="flex max-w-md flex-col gap-1">
        <p className="text-sm font-medium text-foreground">{title}</p>
        {description && <div className="text-[13px] text-foreground-light">{description}</div>}
      </div>
      {actions && <div className="mt-1 flex flex-wrap items-center justify-center gap-2">{actions}</div>}
      {children}
    </div>
  )
}

interface ErrorStateProps {
  error: unknown
  title?: React.ReactNode
  onRetry?: () => void
  retrying?: boolean
  className?: string
}

/** Error box showing the API's message, with an optional Retry. */
export function ErrorState({ error, title = 'Something went wrong', onRetry, retrying, className }: ErrorStateProps) {
  return (
    <div
      role="alert"
      className={cn(
        'flex flex-col items-start gap-3 rounded-lg border border-destructive-border bg-destructive-soft p-4 sm:flex-row sm:items-center',
        className,
      )}
    >
      <AlertTriangle className="size-4 shrink-0 text-destructive" aria-hidden="true" />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <p className="text-sm font-medium text-foreground">{title}</p>
        <p className="text-[13px] break-words text-foreground-light">{errorMessage(error)}</p>
      </div>
      {onRetry && (
        <Button size="tiny" icon={<RefreshCw />} onClick={onRetry} loading={retrying}>
          Retry
        </Button>
      )}
    </div>
  )
}

/** Inline callout (info / warning / destructive / success). */
export function Callout({
  tone = 'info',
  icon,
  title,
  children,
  actions,
  className,
}: {
  tone?: 'info' | 'warning' | 'destructive' | 'success' | 'neutral'
  icon?: React.ReactNode
  title?: React.ReactNode
  children?: React.ReactNode
  actions?: React.ReactNode
  className?: string
}) {
  const toneClass = {
    info: 'border-info-border bg-info-soft [&_[data-icon]]:text-info',
    warning: 'border-warning-border bg-warning-soft [&_[data-icon]]:text-warning',
    destructive: 'border-destructive-border bg-destructive-soft [&_[data-icon]]:text-destructive',
    success: 'border-primary/30 bg-primary-soft [&_[data-icon]]:text-primary',
    neutral: 'border-border-strong bg-surface-200 [&_[data-icon]]:text-foreground-lighter',
  }[tone]
  return (
    <div role={tone === 'destructive' ? 'alert' : 'note'} className={cn('flex gap-3 rounded-lg border p-3.5 text-[13px]', toneClass, className)}>
      {icon && (
        <span data-icon aria-hidden="true" className="mt-0.5 shrink-0 [&_svg]:size-4">
          {icon}
        </span>
      )}
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        {title && <p className="text-sm font-medium text-foreground">{title}</p>}
        {children && <div className="text-foreground-light">{children}</div>}
        {actions && <div className="mt-2 flex flex-wrap gap-2">{actions}</div>}
      </div>
    </div>
  )
}
