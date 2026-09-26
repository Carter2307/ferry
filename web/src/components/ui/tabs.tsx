import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Tabs as TabsPrimitive } from 'radix-ui'

import { cn } from '@/lib/utils'

function Tabs({ className, orientation = 'horizontal', ...props }: React.ComponentProps<typeof TabsPrimitive.Root>) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      data-orientation={orientation}
      orientation={orientation}
      className={cn('group/tabs flex gap-4 data-[orientation=horizontal]:flex-col', className)}
      {...props}
    />
  )
}

/**
 * `underline` (default) = Supabase page tabs: text triggers on a bottom
 * border, the active one gets a foreground underline. `pills` = segmented
 * control on a faint fill.
 */
const tabsListVariants = cva('group/tabs-list inline-flex items-center text-foreground-lighter', {
  variants: {
    variant: {
      underline: 'h-10 w-full justify-start gap-5 overflow-x-auto border-b scrollbar-none',
      pills: 'h-[30px] w-fit gap-0.5 rounded-md border bg-surface-200 p-0.5',
    },
  },
  defaultVariants: { variant: 'underline' },
})

function TabsList({
  className,
  variant = 'underline',
  ...props
}: React.ComponentProps<typeof TabsPrimitive.List> & VariantProps<typeof tabsListVariants>) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      data-variant={variant}
      className={cn(tabsListVariants({ variant }), className)}
      {...props}
    />
  )
}

function TabsTrigger({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        'relative inline-flex cursor-pointer items-center justify-center gap-1.5 text-sm whitespace-nowrap transition-colors outline-none',
        'hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50',
        '[&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*=\'size-\'])]:size-4',
        // underline variant
        'group-data-[variant=underline]/tabs-list:h-full group-data-[variant=underline]/tabs-list:rounded-none group-data-[variant=underline]/tabs-list:px-0.5',
        'group-data-[variant=underline]/tabs-list:after:absolute group-data-[variant=underline]/tabs-list:after:inset-x-0 group-data-[variant=underline]/tabs-list:after:-bottom-px group-data-[variant=underline]/tabs-list:after:h-px group-data-[variant=underline]/tabs-list:after:bg-foreground group-data-[variant=underline]/tabs-list:after:opacity-0',
        'group-data-[variant=underline]/tabs-list:data-[state=active]:text-foreground group-data-[variant=underline]/tabs-list:data-[state=active]:after:opacity-100',
        // pills variant
        'group-data-[variant=pills]/tabs-list:h-full group-data-[variant=pills]/tabs-list:rounded-[5px] group-data-[variant=pills]/tabs-list:px-2.5 group-data-[variant=pills]/tabs-list:text-[13px]',
        'group-data-[variant=pills]/tabs-list:data-[state=active]:bg-surface-100 group-data-[variant=pills]/tabs-list:data-[state=active]:text-foreground group-data-[variant=pills]/tabs-list:data-[state=active]:shadow-card dark:group-data-[variant=pills]/tabs-list:data-[state=active]:bg-surface-300',
        className,
      )}
      {...props}
    />
  )
}

function TabsContent({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return <TabsPrimitive.Content data-slot="tabs-content" className={cn('flex-1 outline-none', className)} {...props} />
}

export { Tabs, TabsList, TabsTrigger, TabsContent, tabsListVariants }
