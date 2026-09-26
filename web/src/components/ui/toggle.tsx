import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"
import { Toggle as TogglePrimitive } from "radix-ui"

const toggleVariants = cva(
  "inline-flex cursor-pointer items-center justify-center gap-1.5 rounded-md text-[13px] font-medium whitespace-nowrap text-foreground-lighter transition-colors outline-none hover:bg-surface-200 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 data-[state=on]:bg-foreground/[0.08] data-[state=on]:text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "bg-transparent",
        outline:
          "border border-border-strong bg-surface-100 hover:bg-surface-200 data-[state=on]:bg-surface-200 dark:data-[state=on]:bg-surface-300",
      },
      size: {
        default: "h-[30px] min-w-[30px] px-2",
        sm: "h-[26px] min-w-[26px] px-1.5 text-xs",
        lg: "h-[34px] min-w-[34px] px-2.5",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Toggle({
  className,
  variant,
  size,
  ...props
}: React.ComponentProps<typeof TogglePrimitive.Root> &
  VariantProps<typeof toggleVariants>) {
  return (
    <TogglePrimitive.Root
      data-slot="toggle"
      className={cn(toggleVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Toggle, toggleVariants }
