import * as React from 'react'

import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { cn } from '@/lib/utils'

interface FormCardProps extends Omit<React.ComponentProps<'form'>, 'title'> {
  /** Optional header row (text on the left, `headerActions` on the right). */
  title?: React.ReactNode
  description?: React.ReactNode
  headerActions?: React.ReactNode
  /** Footer row (usually <FormActions/>). */
  footer?: React.ReactNode
  /** Render a <div> instead of a <form> (read-only cards). */
  asDiv?: boolean
}

/**
 * Supabase "form-item-layout" card: rows of label/description on the left and
 * a control on the right, separated by hairlines, with a Save footer.
 */
export function FormCard({
  title,
  description,
  headerActions,
  footer,
  asDiv = false,
  className,
  children,
  ...props
}: FormCardProps) {
  const body = (
    <>
      {(title || headerActions) && (
        <div className="flex flex-col gap-3 border-b px-5 py-4 sm:flex-row sm:items-center sm:justify-between md:px-6">
          <div className="flex min-w-0 flex-col gap-0.5">
            {title && <h3 className="text-sm font-medium text-foreground">{title}</h3>}
            {description && <p className="text-[13px] text-foreground-light">{description}</p>}
          </div>
          {headerActions && <div className="flex shrink-0 items-center gap-2">{headerActions}</div>}
        </div>
      )}
      <div className="divide-y">{children}</div>
      {footer && (
        <div className="flex flex-wrap items-center justify-end gap-2 border-t bg-surface-75 px-5 py-3 md:px-6 dark:bg-transparent">
          {footer}
        </div>
      )}
    </>
  )
  return (
    <Card className={cn('gap-0', className)}>
      {asDiv ? body : <form noValidate {...props}>{body}</form>}
    </Card>
  )
}

interface FormRowProps {
  label: React.ReactNode
  /** Help text under the label (fg-light). */
  description?: React.ReactNode
  /** id of the control (label `for`). */
  htmlFor?: string
  /** Validation error under the control. */
  error?: React.ReactNode
  /** `horizontal` (default) = label left / control right on ≥md; `vertical` = stacked. */
  layout?: 'horizontal' | 'vertical'
  className?: string
  /** Right column max width (default ~420px). */
  controlClassName?: string
  children: React.ReactNode
}

/** One row of a FormCard. */
export function FormRow({
  label,
  description,
  htmlFor,
  error,
  layout = 'horizontal',
  className,
  controlClassName,
  children,
}: FormRowProps) {
  const LabelTag = htmlFor ? 'label' : 'div'
  return (
    <div
      className={cn(
        'grid gap-3 px-5 py-5 md:px-6',
        layout === 'horizontal' ? 'md:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)] md:gap-8' : '',
        className,
      )}
    >
      <div className="flex min-w-0 flex-col gap-1">
        <LabelTag htmlFor={htmlFor} className="text-sm font-medium text-foreground">
          {label}
        </LabelTag>
        {description && <div className="text-[13px] leading-relaxed text-foreground-light">{description}</div>}
      </div>
      <div className={cn('flex min-w-0 flex-col gap-1.5', controlClassName)}>
        {children}
        {error && (
          <p role="alert" className="text-[13px] text-destructive">
            {error}
          </p>
        )}
      </div>
    </div>
  )
}

interface FormActionsProps {
  /** Enables Save / Cancel. */
  dirty: boolean
  saving?: boolean
  onReset?: () => void
  saveLabel?: string
  /** Extra content on the left (e.g. a "Save & restart" checkbox). */
  children?: React.ReactNode
}

/** Card footer: [children] … Cancel · Save changes (submit). */
export function FormActions({ dirty, saving = false, onReset, saveLabel = 'Save changes', children }: FormActionsProps) {
  return (
    <>
      {children && <div className="mr-auto flex items-center gap-2">{children}</div>}
      {onReset && (
        <Button type="button" variant="default" disabled={!dirty || saving} onClick={onReset}>
          Cancel
        </Button>
      )}
      <Button type="submit" variant="primary" disabled={!dirty} loading={saving}>
        {saveLabel}
      </Button>
    </>
  )
}
