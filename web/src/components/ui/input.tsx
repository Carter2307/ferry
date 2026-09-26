import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'

import { cn } from '@/lib/utils'

const inputVariants = cva(
  [
    'w-full min-w-0 rounded-md border border-border-strong bg-surface-100 text-foreground dark:bg-surface-200',
    'placeholder:text-foreground-muted selection:bg-primary-soft',
    'transition-[border-color,box-shadow] outline-none',
    'hover:border-border-stronger focus-visible:border-primary-bright/70 focus-visible:ring-2 focus-visible:ring-ring',
    'disabled:cursor-not-allowed disabled:opacity-60',
    'read-only:bg-surface-200 read-only:focus-visible:ring-1 dark:read-only:bg-surface-200/60',
    'aria-invalid:border-destructive aria-invalid:ring-destructive/20',
    'file:mr-3 file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground',
  ],
  {
    variants: {
      size: {
        tiny: 'h-[26px] px-2 text-xs',
        sm: 'h-[30px] px-2.5 text-[13px]',
        md: 'h-[34px] px-3 text-sm',
        lg: 'h-[38px] px-3 text-sm',
      },
      mono: {
        true: 'font-mono text-[13px]',
        false: '',
      },
    },
    defaultVariants: { size: 'md', mono: false },
  },
)

type InputProps = Omit<React.ComponentProps<'input'>, 'size'> & VariantProps<typeof inputVariants>

function Input({ className, type, size, mono, ...props }: InputProps) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(inputVariants({ size, mono }), className)}
      {...props}
    />
  )
}

export { Input, inputVariants, type InputProps }
