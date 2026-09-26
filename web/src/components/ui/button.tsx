import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Loader2 } from 'lucide-react'
import { Slot } from 'radix-ui'

import { cn } from '@/lib/utils'

/**
 * Supabase Studio buttons: small, medium-weight, 6px radius, 1px control
 * border. Sizes: tiny (26px), sm (30px, default), md (34px), lg (38px) and
 * square icon sizes. `pill` rounds them fully (top-bar "Connect", search).
 */
const buttonVariants = cva(
  [
    'relative inline-flex shrink-0 cursor-pointer items-center justify-center gap-1.5 rounded-md border font-medium whitespace-nowrap select-none',
    'transition-[color,background-color,border-color,box-shadow,opacity] duration-150 outline-none',
    'focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-0',
    'disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50',
    '[&_svg]:pointer-events-none [&_svg]:shrink-0',
  ],
  {
    variants: {
      variant: {
        /** Outlined neutral button on surface-100. */
        default:
          'border-border-strong bg-surface-100 text-foreground hover:border-border-stronger hover:bg-surface-200 data-[state=open]:bg-surface-200',
        /** Solid green. */
        primary:
          'border-primary-solid-border bg-primary-solid text-primary-foreground hover:brightness-110 dark:hover:brightness-115 disabled:opacity-45',
        /** Transparent with a border (secondary actions on tinted surfaces). */
        outline:
          'border-border-strong bg-transparent text-foreground hover:border-border-stronger hover:bg-surface-200 data-[state=open]:bg-surface-200',
        /** Borderless, subtle hover. */
        ghost:
          'border-transparent bg-transparent text-foreground-light hover:bg-surface-200 hover:text-foreground data-[state=open]:bg-surface-200',
        /** Outlined red ink. */
        danger:
          'border-destructive-border bg-destructive-soft text-destructive hover:border-destructive hover:bg-destructive/10',
        /** Solid red — confirm buttons of destructive dialogs. */
        'danger-solid':
          'border-destructive-solid bg-destructive-solid text-white hover:brightness-110 focus-visible:ring-destructive/40',
        /** Amber outlined. */
        warning:
          'border-warning-border bg-warning-soft text-warning hover:border-warning hover:brightness-105',
        /** Text link look. */
        link: 'h-auto border-transparent bg-transparent px-0 text-primary underline-offset-4 hover:underline',
        /** Dashed border (filter buttons in toolbars). */
        dashed:
          'border-dashed border-border-stronger bg-transparent text-foreground-light hover:border-foreground-muted hover:text-foreground data-[state=open]:bg-surface-200',
      },
      size: {
        tiny: 'h-[26px] gap-1 px-2 text-xs [&_svg:not([class*=size-])]:size-3.5',
        sm: 'h-[30px] px-2.5 text-[13px] [&_svg:not([class*=size-])]:size-4',
        md: 'h-[34px] px-3 text-sm [&_svg:not([class*=size-])]:size-4',
        lg: 'h-[38px] px-4 text-sm [&_svg:not([class*=size-])]:size-4',
        icon: 'size-[30px] px-0 [&_svg:not([class*=size-])]:size-4',
        'icon-tiny': 'size-[26px] px-0 [&_svg:not([class*=size-])]:size-3.5',
        'icon-md': 'size-[34px] px-0 [&_svg:not([class*=size-])]:size-4',
        'icon-lg': 'size-8 px-0 [&_svg:not([class*=size-])]:size-4',
      },
      shape: {
        default: '',
        pill: 'rounded-full',
      },
    },
    compoundVariants: [{ variant: 'link', className: 'h-auto px-0' }],
    defaultVariants: { variant: 'default', size: 'sm', shape: 'default' },
  },
)

type ButtonProps = React.ComponentProps<'button'> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
    /** Shows a spinner in place of `icon` and disables the button. */
    loading?: boolean
    /** Leading icon (rendered before children). */
    icon?: React.ReactNode
    /** Trailing icon (rendered after children). */
    iconRight?: React.ReactNode
  }

function Button({
  className,
  variant,
  size,
  shape,
  asChild = false,
  loading = false,
  icon,
  iconRight,
  disabled,
  children,
  type,
  ...props
}: ButtonProps) {
  if (asChild) {
    const cls = cn(buttonVariants({ variant, size, shape }), className)
    if (!icon && !iconRight) {
      return (
        <Slot.Root data-slot="button" className={cls} {...props}>
          {children}
        </Slot.Root>
      )
    }
    // Slottable lets the icons render inside the child element (e.g. an <a>).
    return (
      <Slot.Root data-slot="button" className={cls} {...props}>
        {icon}
        <Slot.Slottable>{children}</Slot.Slottable>
        {iconRight}
      </Slot.Root>
    )
  }
  return (
    <button
      data-slot="button"
      data-variant={variant ?? 'default'}
      type={type ?? 'button'}
      className={cn(buttonVariants({ variant, size, shape }), className)}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...props}
    >
      {loading ? <Loader2 className="animate-spin" aria-hidden="true" /> : icon}
      {children}
      {iconRight}
    </button>
  )
}

export { Button, buttonVariants, type ButtonProps }
