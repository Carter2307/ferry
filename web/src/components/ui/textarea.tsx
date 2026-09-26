import * as React from 'react'

import { cn } from '@/lib/utils'

type TextareaProps = React.ComponentProps<'textarea'> & { mono?: boolean }

function Textarea({ className, mono = false, ...props }: TextareaProps) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        'flex field-sizing-content min-h-20 w-full rounded-md border border-border-strong bg-surface-100 px-3 py-2 text-sm text-foreground dark:bg-surface-200',
        'placeholder:text-foreground-muted transition-[border-color,box-shadow] outline-none',
        'hover:border-border-stronger focus-visible:border-primary-bright/70 focus-visible:ring-2 focus-visible:ring-ring',
        'disabled:cursor-not-allowed disabled:opacity-60 aria-invalid:border-destructive aria-invalid:ring-destructive/20',
        mono && 'font-mono text-[13px] leading-relaxed',
        className,
      )}
      {...props}
    />
  )
}

export { Textarea, type TextareaProps }
