import * as React from 'react'

import { cn } from '@/lib/utils'

const widths = {
  /** Settings-like pages (~800px centered column). */
  narrow: 'max-w-[848px]',
  /** Overview pages (~1200px). */
  default: 'max-w-[1248px]',
  /** Full width (logs). */
  full: 'max-w-none',
} as const

/** Page content column with Studio side padding. */
export function PageContainer({
  size = 'default',
  className,
  ...props
}: React.ComponentProps<'div'> & { size?: keyof typeof widths }) {
  return (
    <div
      className={cn('mx-auto w-full px-4 pt-6 pb-16 sm:px-6 md:pt-8 lg:px-10', widths[size], className)}
      {...props}
    />
  )
}

interface PageHeaderProps {
  title: React.ReactNode
  /** Subtitle under the title (fg-light). */
  description?: React.ReactNode
  /** Pills next to the title (e.g. `BETA`, a state pill). */
  badges?: React.ReactNode
  /** Right-aligned toolbar (Docs, Refresh, primary action…). */
  actions?: React.ReactNode
  /** Small line above the title (breadcrumb / back link). */
  eyebrow?: React.ReactNode
  /** `lg` = 32px entity title (service home), `md` = 26px page title. */
  size?: 'md' | 'lg'
  className?: string
  children?: React.ReactNode
}

/** Page title block: title (+badges) + subtitle on the left, actions on the right. */
export function PageHeader({ title, description, badges, actions, eyebrow, size = 'md', className, children }: PageHeaderProps) {
  return (
    <header className={cn('mb-6 flex flex-col gap-4 md:mb-8', className)}>
      {eyebrow && <div className="text-[13px] text-foreground-lighter">{eyebrow}</div>}
      <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div className="flex min-w-0 flex-col gap-1.5">
          <div className="flex min-w-0 flex-wrap items-center gap-2.5">
            <h1
              className={cn(
                'min-w-0 truncate font-medium tracking-[-0.01em] text-foreground',
                size === 'lg' ? 'text-[26px] leading-tight md:text-[32px]' : 'text-2xl leading-tight md:text-[26px]',
              )}
            >
              {title}
            </h1>
            {badges}
          </div>
          {description && (
            <div className={cn('text-foreground-light', size === 'lg' ? 'text-base' : 'text-sm')}>{description}</div>
          )}
        </div>
        {actions && <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {children}
    </header>
  )
}

interface PageSectionProps extends Omit<React.ComponentProps<'section'>, 'title'> {
  title?: React.ReactNode
  description?: React.ReactNode
  actions?: React.ReactNode
  /** Section id for in-page anchors. */
  id?: string
}

/** A titled section of a page (20px/500 heading, optional description + actions). */
export function PageSection({ title, description, actions, className, children, ...props }: PageSectionProps) {
  const headingId = React.useId()
  return (
    <section aria-labelledby={title ? headingId : undefined} className={cn('mb-10 flex flex-col gap-4 last:mb-0', className)} {...props}>
      {(title || actions) && (
        <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
          <div className="flex min-w-0 flex-col gap-1">
            {title && (
              <h2 id={headingId} className="text-lg font-medium text-foreground md:text-xl">
                {title}
              </h2>
            )}
            {description && <p className="text-sm text-foreground-light">{description}</p>}
          </div>
          {actions && <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>}
        </div>
      )}
      {children}
    </section>
  )
}
