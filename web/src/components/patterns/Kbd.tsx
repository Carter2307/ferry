import * as React from 'react'

import { cn } from '@/lib/utils'

/** Keyboard hint (⌘K). */
export function Kbd({ className, ...props }: React.ComponentProps<'kbd'>) {
  return (
    <kbd
      className={cn(
        'inline-flex h-[18px] min-w-[18px] items-center justify-center rounded-[4px] border border-border-strong bg-surface-200 px-1 font-mono text-[10.5px] leading-none text-foreground-lighter',
        className,
      )}
      {...props}
    />
  )
}
