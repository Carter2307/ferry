import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Slot } from 'radix-ui'

import { cn } from '@/lib/utils'

/**
 * Supabase pills: tiny uppercase labels with a 1px border, rounded-full.
 * `mono` switches to the monospace face (e.g. `NANO`, runtime tags);
 * `square` gives the 4px-radius chip look.
 */
const badgeVariants = cva(
  [
    'inline-flex h-5 w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-full border px-2',
    'text-[10.5px] leading-none font-medium tracking-[0.04em] whitespace-nowrap uppercase',
    'focus-visible:ring-2 focus-visible:ring-ring outline-none',
    '[&>svg]:pointer-events-none [&>svg]:size-3',
  ],
  {
    variants: {
      variant: {
        default: 'border-border-strong bg-surface-200 text-foreground-light',
        outline: 'border-border-strong bg-transparent text-foreground-light',
        success: 'border-success/30 bg-success-soft text-success',
        warning: 'border-warning-border bg-warning-soft text-warning',
        destructive: 'border-destructive-border bg-destructive-soft text-destructive',
        info: 'border-info-border bg-info-soft text-info',
        brand: 'border-primary-solid-border bg-primary-solid text-primary-foreground',
      },
      font: {
        sans: '',
        mono: 'font-mono text-[10.5px] font-normal tracking-[0.06em]',
      },
      shape: {
        pill: 'rounded-full',
        square: 'rounded-sm',
      },
      case: {
        upper: 'uppercase',
        normal: 'normal-case tracking-normal',
      },
    },
    defaultVariants: { variant: 'default', font: 'sans', shape: 'pill', case: 'upper' },
  },
)

type BadgeProps = React.ComponentProps<'span'> & VariantProps<typeof badgeVariants> & { asChild?: boolean }

function Badge({ className, variant, font, shape, case: letterCase, asChild = false, ...props }: BadgeProps) {
  const Comp = asChild ? Slot.Root : 'span'
  return (
    <Comp
      data-slot="badge"
      data-variant={variant ?? 'default'}
      className={cn(badgeVariants({ variant, font, shape, case: letterCase }), className)}
      {...props}
    />
  )
}

export { Badge, badgeVariants, type BadgeProps }
