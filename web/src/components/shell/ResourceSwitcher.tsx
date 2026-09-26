import * as React from 'react'
import { useNavigate } from 'react-router'
import { Check, ChevronsUpDown } from 'lucide-react'

import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator } from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'

export interface SwitcherItem {
  value: string
  label: string
  to: string
  icon?: React.ReactNode
  /** Right-aligned meta (state dot, kind). */
  meta?: React.ReactNode
}

interface ResourceSwitcherProps {
  /** Current item value (checked). */
  current?: string
  items: SwitcherItem[]
  placeholder: string
  /** Accessible label for the trigger ("Switch service"). */
  label: string
  /** Links at the bottom ("All services", "+ New service"). */
  footer?: { label: string; to: string; icon?: React.ReactNode }[]
  loading?: boolean
  /** Trigger content (segment text). */
  children: React.ReactNode
  className?: string
}

/** Breadcrumb segment with a ⇕ searchable switcher popover (Studio project/branch selector). */
export function ResourceSwitcher({ current, items, placeholder, label, footer, loading, children, className }: ResourceSwitcherProps) {
  const [open, setOpen] = React.useState(false)
  const navigate = useNavigate()
  const go = (to: string) => {
    setOpen(false)
    void navigate(to)
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={label}
          className={cn(
            'group inline-flex h-8 min-w-0 cursor-pointer items-center gap-1.5 rounded-md px-1.5 text-sm text-foreground outline-none transition-colors',
            'hover:bg-surface-200 focus-visible:ring-2 focus-visible:ring-ring data-[state=open]:bg-surface-200',
            className,
          )}
        >
          {children}
          <ChevronsUpDown className="size-3.5 shrink-0 text-foreground-lighter group-hover:text-foreground-light" aria-hidden="true" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 p-0">
        <Command loop>
          <CommandInput placeholder={placeholder} />
          <CommandList className="max-h-80">
            <CommandEmpty>{loading ? 'Loading…' : 'Nothing found.'}</CommandEmpty>
            <CommandGroup>
              {items.map((item) => (
                <CommandItem key={item.value} value={`${item.label} ${item.value}`} onSelect={() => go(item.to)}>
                  {item.icon}
                  <span className="truncate">{item.label}</span>
                  <span className="ml-auto flex items-center gap-2">
                    {item.meta}
                    <Check className={cn('size-3.5', item.value === current ? 'text-foreground' : 'invisible')} aria-hidden="true" />
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
            {footer && footer.length > 0 && (
              <>
                <CommandSeparator />
                <CommandGroup>
                  {footer.map((f) => (
                    <CommandItem key={f.to} value={`__footer ${f.label}`} onSelect={() => go(f.to)}>
                      {f.icon}
                      {f.label}
                    </CommandItem>
                  ))}
                </CommandGroup>
              </>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}
