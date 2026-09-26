import * as React from 'react'
import { Switch as SwitchPrimitive } from 'radix-ui'

import { cn } from '@/lib/utils'

function Switch({
  className,
  size = 'default',
  ...props
}: React.ComponentProps<typeof SwitchPrimitive.Root> & { size?: 'sm' | 'default' }) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      data-size={size}
      className={cn(
        'peer group/switch inline-flex shrink-0 cursor-pointer items-center rounded-full border transition-colors outline-none',
        'focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50',
        'data-[size=default]:h-5 data-[size=default]:w-[34px] data-[size=sm]:h-4 data-[size=sm]:w-7',
        'data-[state=checked]:border-primary-solid-border data-[state=checked]:bg-primary-solid data-[state=unchecked]:border-border-strong data-[state=unchecked]:bg-surface-200',
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className={cn(
          'pointer-events-none block rounded-full bg-white shadow-sm ring-0 transition-transform',
          'group-data-[size=default]/switch:size-4 group-data-[size=sm]/switch:size-3',
          'data-[state=checked]:translate-x-[15px] data-[state=unchecked]:translate-x-px group-data-[size=sm]/switch:data-[state=checked]:translate-x-[13px]',
          'dark:data-[state=unchecked]:bg-foreground-light',
        )}
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
